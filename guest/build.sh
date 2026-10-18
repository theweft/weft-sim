#!/bin/sh
# SPDX-FileCopyrightText: 2026 The weft authors
# SPDX-License-Identifier: MIT
#
# Packs the test initramfs. Inputs come from the environment (see the justfile):
#   BUSYBOX  static busybox binary      MODULES  kernel modules directory (lib/modules/<ver>)
#   RUNNER   static weft-sim binary     SIM_KO   built weft_sim.ko
#   BASE     base overlay (.dtbo)       BOARD    board overlay (.dtbo)
#   OUT      initramfs file to write
set -eu
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

mkdir -p "$root/bin" "$root/lib/modules" "$root/boot" "$root/proc" "$root/sys" "$root/dev"
cp "$BUSYBOX" "$root/bin/busybox"
for applet in $("$BUSYBOX" --list); do ln -sf busybox "$root/bin/$applet"; done
cp "$RUNNER" "$root/bin/weft-sim"

# Drivers under test, in load order.
order="industrialio kfifo_buf industrialio-triggered-buffer ti-ads1015"
for m in $order; do
	src=$(find "$MODULES" -name "$m.ko*" | head -n1)
	case "$src" in
	*.xz) xz -dc "$src" >"$root/lib/modules/$m.ko" ;;
	*.zst) zstd -dcq "$src" >"$root/lib/modules/$m.ko" ;;
	*) cp "$src" "$root/lib/modules/$m.ko" ;;
	esac
done
cp "$SIM_KO" "$root/lib/modules/weft_sim.ko"
echo "$order weft_sim" >"$root/lib/modules/order"

cp "$BASE" "$root/boot/base.dtbo"
cp "$BOARD" "$root/boot/board.dtbo"
install -m 0755 "$(dirname "$0")/init" "$root/init"

(cd "$root" && find . | cpio -o -H newc --quiet) | gzip -9 >"$OUT"
