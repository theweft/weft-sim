/* SPDX-FileCopyrightText: 2026 The weft authors */
/* SPDX-License-Identifier: MIT OR GPL-2.0-only */
/*
 * Interface between the weft-sim kernel module and the user-space runner.
 *
 * read() on /dev/weft-sim returns one transfer request:
 *   struct weft_sim_req, then nmsgs x struct weft_sim_msg, then the bytes of all write
 *   messages in order.
 * write() answers it:
 *   struct weft_sim_rep, then (if status is 0) the bytes of all read messages in order.
 * All fields are in the host's byte order.
 */
#ifndef WEFT_SIM_H
#define WEFT_SIM_H

#include <linux/ioctl.h>
#include <linux/types.h>

/* Largest request or reply, header included. */
#define WEFT_SIM_MAX_XFER 8192
/* Longest device tree path accepted by WEFT_SIM_ADD_I2C. */
#define WEFT_SIM_PATH_MAX 128

struct weft_sim_req {
	__u32 seq;   /* echoed in the reply */
	__u16 bus;   /* I2C adapter number */
	__u16 nmsgs;
};

struct weft_sim_msg {
	__u16 addr;
	__u16 flags; /* I2C_M_* from <linux/i2c.h>; I2C_M_RD = 0x0001 */
	__u16 len;
	__u16 reserved;
};

struct weft_sim_rep {
	__u32 seq;
	__s32 status; /* 0, or a negative errno: -ENXIO when nobody acknowledged */
};

struct weft_sim_overlay {
	__u64 data; /* user pointer to a flattened device tree overlay */
	__u32 len;
	__u32 reserved;
};

struct weft_sim_i2c {
	__u32 nr; /* adapter number, /dev/i2c-<nr> */
	char path[WEFT_SIM_PATH_MAX]; /* device tree node the adapter belongs to */
};

#define WEFT_SIM_IOC_APPLY_OVERLAY _IOW('W', 1, struct weft_sim_overlay)
#define WEFT_SIM_IOC_ADD_I2C _IOW('W', 2, struct weft_sim_i2c)

#endif
