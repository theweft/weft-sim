// SPDX-FileCopyrightText: 2026 The weft authors
// SPDX-License-Identifier: MIT OR GPL-2.0-only
/*
 * weft-sim: I2C adapters whose devices are simulated in user space.
 *
 * The module only forwards. Each transfer on a weft-sim adapter becomes one request on
 * /dev/weft-sim; the runner (`weft-sim serve`) answers it from its chip models. Two ioctls let the
 * runner apply device tree overlays and register adapters for device tree nodes, so the real
 * upstream drivers bind through the same overlays as on hardware.
 */

#include <linux/completion.h>
#include <linux/fs.h>
#include <linux/i2c.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/of_platform.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <linux/wait.h>

#include "weft_sim.h"

#define WEFT_SIM_MAX_ADAPTERS 8
#define WEFT_SIM_MAX_OVERLAYS 16

static unsigned int timeout_ms = 1000;
module_param(timeout_ms, uint, 0644);
MODULE_PARM_DESC(timeout_ms, "How long a transfer waits for the runner");

/* Serialises transfers across all adapters: one request is in flight at a time. */
static DEFINE_MUTEX(xfer_lock);
/* Protects the request/reply state below. */
static DEFINE_MUTEX(state_lock);
static DECLARE_WAIT_QUEUE_HEAD(req_wait);
static DECLARE_COMPLETION(rep_done);

static atomic_t runner_open = ATOMIC_INIT(0);
static u8 req_buf[WEFT_SIM_MAX_XFER];
static size_t req_len;
static bool req_ready; /* a request waits to be read */
static bool req_taken; /* the runner has read it and owes a reply */
static u32 req_seq;
static u8 rep_buf[WEFT_SIM_MAX_XFER];
static size_t rep_len;
static int rep_status;

static DEFINE_MUTEX(config_lock);
static struct i2c_adapter *adapters[WEFT_SIM_MAX_ADAPTERS];
static int overlays[WEFT_SIM_MAX_OVERLAYS];
static int n_overlays;

/**
 * @relation(SIM-001, scope=function)
 */
static int weft_sim_xfer(struct i2c_adapter *adap, struct i2c_msg *msgs, int num)
{
	struct weft_sim_req *req = (struct weft_sim_req *)req_buf;
	struct weft_sim_msg *desc;
	size_t len, off;
	int i, ret;

	if (num <= 0 || num > U16_MAX)
		return -EINVAL;

	mutex_lock(&xfer_lock);
	if (!atomic_read(&runner_open)) {
		ret = -ENODEV;
		goto out;
	}

	len = sizeof(*req) + num * sizeof(*desc);
	for (i = 0; i < num; i++)
		if (!(msgs[i].flags & I2C_M_RD))
			len += msgs[i].len;
	if (len > WEFT_SIM_MAX_XFER) {
		ret = -EMSGSIZE;
		goto out;
	}

	mutex_lock(&state_lock);
	req->seq = ++req_seq;
	req->bus = adap->nr;
	req->nmsgs = num;
	desc = (struct weft_sim_msg *)(req + 1);
	off = sizeof(*req) + num * sizeof(*desc);
	for (i = 0; i < num; i++) {
		desc[i].addr = msgs[i].addr;
		desc[i].flags = msgs[i].flags;
		desc[i].len = msgs[i].len;
		desc[i].reserved = 0;
		if (!(msgs[i].flags & I2C_M_RD)) {
			memcpy(req_buf + off, msgs[i].buf, msgs[i].len);
			off += msgs[i].len;
		}
	}
	req_len = len;
	req_ready = true;
	req_taken = false;
	reinit_completion(&rep_done);
	mutex_unlock(&state_lock);
	wake_up_interruptible(&req_wait);

	if (!wait_for_completion_timeout(&rep_done, msecs_to_jiffies(timeout_ms))) {
		mutex_lock(&state_lock);
		req_ready = false;
		req_taken = false;
		mutex_unlock(&state_lock);
		ret = -ETIMEDOUT;
		goto out;
	}

	mutex_lock(&state_lock);
	ret = rep_status;
	if (!ret) {
		off = 0;
		for (i = 0; i < num; i++) {
			if (!(msgs[i].flags & I2C_M_RD))
				continue;
			if (off + msgs[i].len > rep_len) {
				ret = -EPROTO;
				break;
			}
			memcpy(msgs[i].buf, rep_buf + off, msgs[i].len);
			off += msgs[i].len;
		}
		if (!ret)
			ret = num;
	}
	mutex_unlock(&state_lock);
out:
	mutex_unlock(&xfer_lock);
	return ret;
}

static u32 weft_sim_func(struct i2c_adapter *adap)
{
	return I2C_FUNC_I2C | I2C_FUNC_SMBUS_EMUL;
}

static const struct i2c_algorithm weft_sim_algo = {
	.xfer = weft_sim_xfer,
	.functionality = weft_sim_func,
};

static int weft_sim_open(struct inode *inode, struct file *file)
{
	if (atomic_cmpxchg(&runner_open, 0, 1))
		return -EBUSY;
	return 0;
}

static int weft_sim_release(struct inode *inode, struct file *file)
{
	mutex_lock(&state_lock);
	atomic_set(&runner_open, 0);
	if (req_ready) {
		req_ready = false;
		rep_status = -ENODEV;
		complete(&rep_done);
	}
	mutex_unlock(&state_lock);
	return 0;
}

static ssize_t weft_sim_read(struct file *file, char __user *buf, size_t count, loff_t *ppos)
{
	ssize_t ret;

	ret = wait_event_interruptible(req_wait, req_ready && !req_taken);
	if (ret)
		return ret;
	mutex_lock(&state_lock);
	if (!req_ready || req_taken) {
		ret = -EAGAIN;
	} else if (count < req_len) {
		ret = -EMSGSIZE;
	} else if (copy_to_user(buf, req_buf, req_len)) {
		ret = -EFAULT;
	} else {
		req_taken = true;
		ret = req_len;
	}
	mutex_unlock(&state_lock);
	return ret;
}

static ssize_t weft_sim_write(struct file *file, const char __user *buf, size_t count,
			      loff_t *ppos)
{
	struct weft_sim_rep rep;
	ssize_t ret = count;

	if (count < sizeof(rep) || count > WEFT_SIM_MAX_XFER)
		return -EINVAL;
	if (copy_from_user(&rep, buf, sizeof(rep)))
		return -EFAULT;

	mutex_lock(&state_lock);
	if (!req_ready || !req_taken || rep.seq != req_seq) {
		ret = -ESTALE; /* the transfer timed out or was never read */
	} else if (copy_from_user(rep_buf, buf + sizeof(rep), count - sizeof(rep))) {
		ret = -EFAULT;
	} else {
		rep_len = count - sizeof(rep);
		rep_status = rep.status > 0 ? -EPROTO : rep.status;
		req_ready = false;
		req_taken = false;
		complete(&rep_done);
	}
	mutex_unlock(&state_lock);
	return ret;
}

/*
 * Board-level devices that are not on a simulated bus (fixed regulators such as the Pi's
 * vdd_3v3_reg): x86 has no platform bus for device tree nodes, so create platform devices for
 * the children of /weft-sim/regulators. Idempotent; nodes removed with an overlay take their
 * devices with them.
 */
/**
 * @relation(SIM-003, scope=function)
 */
static void weft_sim_populate(void)
{
	struct device_node *np = of_find_node_by_path("/weft-sim/regulators");

	if (!np)
		return;
	if (of_platform_default_populate(np, NULL, NULL))
		pr_warn("weft-sim: creating regulator devices failed\n");
	of_node_put(np);
}

static long weft_sim_apply_overlay(void __user *arg)
{
	struct weft_sim_overlay ov;
	void *fdt;
	int id = 0, ret;

	if (copy_from_user(&ov, arg, sizeof(ov)))
		return -EFAULT;
	if (!ov.len || ov.len > SZ_1M)
		return -EINVAL;
	mutex_lock(&config_lock);
	if (n_overlays == WEFT_SIM_MAX_OVERLAYS) {
		ret = -ENOSPC;
		goto out;
	}
	fdt = memdup_user(u64_to_user_ptr(ov.data), ov.len);
	if (IS_ERR(fdt)) {
		ret = PTR_ERR(fdt);
		goto out;
	}
	/* Probing drivers transfer while this runs; the runner serves them from another thread. */
	ret = of_overlay_fdt_apply(fdt, ov.len, &id, NULL);
	kfree(fdt);
	if (ret) {
		/* A partly applied overlay still has an id that must be removed. */
		if (id)
			of_overlay_remove(&id);
		goto out;
	}
	overlays[n_overlays++] = id;
	weft_sim_populate();
out:
	mutex_unlock(&config_lock);
	return ret;
}

static long weft_sim_add_i2c(void __user *arg)
{
	struct weft_sim_i2c req;
	struct i2c_adapter *adap;
	struct device_node *np;
	int slot, ret;

	if (copy_from_user(&req, arg, sizeof(req)))
		return -EFAULT;
	req.path[WEFT_SIM_PATH_MAX - 1] = '\0';

	np = of_find_node_by_path(req.path);
	if (!np)
		return -ENOENT;

	mutex_lock(&config_lock);
	for (slot = 0; slot < WEFT_SIM_MAX_ADAPTERS && adapters[slot]; slot++)
		;
	if (slot == WEFT_SIM_MAX_ADAPTERS) {
		ret = -ENOSPC;
		goto err_put;
	}
	adap = kzalloc(sizeof(*adap), GFP_KERNEL);
	if (!adap) {
		ret = -ENOMEM;
		goto err_put;
	}
	adap->owner = THIS_MODULE;
	adap->algo = &weft_sim_algo;
	adap->nr = req.nr;
	adap->dev.of_node = np; /* reference kept until the adapter is removed */
	snprintf(adap->name, sizeof(adap->name), "weft-sim i2c %u", req.nr);
	ret = i2c_add_numbered_adapter(adap);
	if (ret) {
		kfree(adap);
		goto err_put;
	}
	adapters[slot] = adap;
	mutex_unlock(&config_lock);
	return 0;

err_put:
	mutex_unlock(&config_lock);
	of_node_put(np);
	return ret;
}

static long weft_sim_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	switch (cmd) {
	case WEFT_SIM_IOC_APPLY_OVERLAY:
		return weft_sim_apply_overlay((void __user *)arg);
	case WEFT_SIM_IOC_ADD_I2C:
		return weft_sim_add_i2c((void __user *)arg);
	default:
		return -ENOTTY;
	}
}

static const struct file_operations weft_sim_fops = {
	.owner = THIS_MODULE,
	.open = weft_sim_open,
	.release = weft_sim_release,
	.read = weft_sim_read,
	.write = weft_sim_write,
	.unlocked_ioctl = weft_sim_ioctl,
	.compat_ioctl = compat_ptr_ioctl,
};

static struct miscdevice weft_sim_dev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = "weft-sim",
	.fops = &weft_sim_fops,
};

static int __init weft_sim_init(void)
{
	return misc_register(&weft_sim_dev);
}

static void __exit weft_sim_exit(void)
{
	int i;

	misc_deregister(&weft_sim_dev);
	for (i = 0; i < WEFT_SIM_MAX_ADAPTERS; i++) {
		if (!adapters[i])
			continue;
		of_node_put(adapters[i]->dev.of_node);
		i2c_del_adapter(adapters[i]);
		kfree(adapters[i]);
	}
	while (n_overlays)
		of_overlay_remove(&overlays[--n_overlays]);
}

module_init(weft_sim_init);
module_exit(weft_sim_exit);

MODULE_DESCRIPTION("I2C adapters backed by user-space chip models (weft-sim)");
MODULE_LICENSE("Dual MIT/GPL");
