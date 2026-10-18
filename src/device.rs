// SPDX-FileCopyrightText: 2026 The weft authors
// SPDX-License-Identifier: MIT

//! The ioctls of `/dev/weft-sim` (see `module/weft_sim.h`). The only unsafe code in the runner.
#![allow(unsafe_code)]

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

/// `WEFT_SIM_MAX_XFER`.
pub const MAX_XFER: usize = 8192;
const PATH_MAX: usize = 128;

#[repr(C)]
struct Overlay {
    data: u64,
    len: u32,
    reserved: u32,
}

#[repr(C)]
struct AddI2c {
    nr: u32,
    path: [u8; PATH_MAX],
}

/// `_IOW('W', nr, size)` in the generic Linux encoding (x86, arm, riscv).
const fn iow(nr: u32, size: usize) -> u32 {
    #[allow(clippy::cast_possible_truncation)]
    let size = size as u32;
    (1 << 30) | (size << 16) | ((b'W' as u32) << 8) | nr
}

const APPLY_OVERLAY: u32 = iow(1, size_of::<Overlay>());
const ADD_I2C: u32 = iow(2, size_of::<AddI2c>());

fn ioctl<T>(device: &File, request: u32, arg: &T) -> io::Result<()> {
    // The request type is `unsigned long` in glibc and `int` in musl; the value fits either.
    #[allow(
        clippy::cast_possible_wrap,
        clippy::cast_lossless,
        clippy::useless_conversion
    )]
    let request = request as libc::Ioctl;
    // SAFETY: `arg` is a live, correctly laid out #[repr(C)] struct matching `request`; the
    // kernel only reads from it (and from the buffer it points to) during the call.
    let ret = unsafe { libc::ioctl(device.as_raw_fd(), request, std::ptr::from_ref(arg)) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Applies a flattened device tree overlay.
///
/// # Errors
/// When the kernel rejects the overlay.
pub fn apply_overlay(device: &File, fdt: &[u8]) -> io::Result<()> {
    let len = u32::try_from(fdt.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let arg = Overlay {
        data: fdt.as_ptr() as u64,
        len,
        reserved: 0,
    };
    ioctl(device, APPLY_OVERLAY, &arg)
}

/// Registers I2C adapter `nr` for the device tree node at `path`.
///
/// # Errors
/// When the path is too long or the kernel cannot register the adapter.
pub fn add_i2c(device: &File, nr: u32, path: &str) -> io::Result<()> {
    let mut arg = AddI2c {
        nr,
        path: [0; PATH_MAX],
    };
    let bytes = path.as_bytes();
    arg.path
        .get_mut(..bytes.len())
        .filter(|_| bytes.len() < PATH_MAX)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
        .copy_from_slice(bytes);
    ioctl(device, ADD_I2C, &arg)
}
