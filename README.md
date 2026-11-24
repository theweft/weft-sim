<h1><img src="https://theweft.org/brand/repos/weft-sim.svg" alt="" width="44" height="44" align="absmiddle"> weft-sim</h1>

**Board simulation.** The real Linux drivers run in a small VM and talk to
simulated chips from [weft-parts](https://github.com/theweft/weft-parts), bound through the same
device tree overlays as on hardware.

> [!NOTE]
> Prototype. One chip (ADS1115) on one simulated I2C bus, x86-64 guest. Like all of weft, it is
> built heavily with AI; see [How weft is made](https://github.com/theweft/weft-design#how-weft-is-made).

## How it works

```
┌──────────────── guest Linux (QEMU, KVM / HVF / WHPX) ──────────┐
│ real drivers: ti-ads1015, at24,    bound by the board overlay  │
│ mcp4728                                                        │
│        │ I2C transfers                                         │
│ weft_sim.ko: I2C adapters that forward each transfer           │
│        │ /dev/weft-sim                                         │
│ weft-sim serve: weft-parts chip models                         │
└────────────────────────────────────────────────────────────────┘
```

1. `weft_sim.ko` (C, about 370 lines) creates `/dev/weft-sim`. It holds no chip logic.
2. `weft-sim serve` (Rust, static binary) attaches the chip models and starts answering transfers.
3. The runner applies the base overlay (`guest/base.dts`), which creates a simulated bus node
   published as `&i2c1` and a fixed 3.3 V regulator published as `&vdd_3v3_reg` (both labels as
   on a Raspberry Pi), registers I2C adapter 1 for the bus, then applies the board overlay,
   exactly as written for a Raspberry Pi (`tests/boards/hat.dts`: ADS1115, 24C32, MCP4728).
4. The upstream drivers probe, and the test drives each chip through sysfs: reads the ADC,
   writes 39 bytes across two EEPROM page boundaries and reads them back (the at24 driver
   retries through each write cycle's NACKs), and stores a DAC code to EEPROM (the mcp4728
   driver polls RDY/BSY through the programming time).

The models run inside the guest, so bus traffic never leaves the VM and the host needs only a stock
QEMU. The guest's CPU follows the host's, so the hypervisor always accelerates it. Why this design
and not vhost-user: weft-design [docs/04-decisions.md](https://github.com/theweft/weft-design/blob/main/docs/04-decisions.md).

The models are time-aware (conversion times, EEPROM write cycles) and take their time from the
guest's monotonic clock. Under the hypervisor that follows real time, so drivers see chips take as
long as on hardware, but runs are not repeatable to the nanosecond. Deterministic, virtual time
belongs to host tests (weft-parts `VirtualClock`) and to a later loosely timed tier.

## Run

```sh
nix develop      # Linux: toolchain, QEMU, guest kernel, busybox
just sim         # build module, runner and initramfs; boot; expect "SIM PASS"
just sim weft.trace    # also log every I2C transfer
```

On a laptop with KVM, a run takes under a second, boot included.

## Layout

| Path | What |
|---|---|
| `module/` | Kernel module and the interface header shared with the runner |
| `src/` | The runner: wire format, ioctls, command line |
| `guest/` | Base overlay, init script, initramfs builder |
| `tests/boards/` | Board overlays under test |

## Next

- Our own minimal guest kernel for x86-64 and arm64, published as a ready-made image.
- Host harness (`weft-sim run`) for Linux, macOS and Windows: picks KVM, HVF or WHPX and steers
  tests over a virtio-serial control channel.
- More chips: SPI (AD5592R), GPIO and interrupts through `gpio-sim` (ADS1115 ALERT/RDY,
  MCP4728 LDAC, 24C32 WP), once weft-parts has simulated nets (weft-design docs/21).

## Licence

MIT, like all weft repositories. The kernel module is MIT OR GPL-2.0-only, so it can also be built
into Linux. See [LICENSE](LICENSE) and [LICENSES/](LICENSES/).
