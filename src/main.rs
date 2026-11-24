// SPDX-FileCopyrightText: 2026 The weft authors
// SPDX-License-Identifier: MIT

//! weft-sim: board simulation with weft-parts chip models.
//!
//! `weft-sim serve` serves the weft-sim kernel module.
//!
//! Runs inside the guest. It attaches models to simulated buses, starts answering transfers,
//! then applies device tree overlays and registers adapters in the order given, so drivers
//! that probe while an overlay is applied are already served.
//!
//! ```text
//! weft-sim serve --chip 1:0x48=ads1115 --input 1:0x48:0=1.25 \
//!     --overlay base.dtbo --i2c 1=/weft-sim/i2c@1 --overlay board.dtbo
//! ```
#![doc(html_logo_url = "https://theweft.org/brand/repos/weft-sim.svg")]
#![doc(html_favicon_url = "https://theweft.org/favicon.svg")]

mod device;
mod wire;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use weft_parts::Library;
use weft_parts::sim::{self, Ads1115, HostClock, I2cBus};

/// A setup step that touches the kernel, run in command-line order.
enum Step {
    Overlay(String),
    I2c(u32, String),
}

struct Config {
    device: String,
    chips: Vec<(u16, u16, String)>,
    inputs: Vec<(u16, u16, usize, f64)>,
    steps: Vec<Step>,
    trace: bool,
}

const USAGE: &str = "usage: weft-sim serve [--device PATH] [--trace] \
[--chip BUS:ADDR=CHIP]... [--input BUS:ADDR:CHANNEL=VOLTS]... \
[--overlay FILE | --i2c NR=DT_PATH]...";

fn number(text: &str) -> Result<u16, String> {
    let parsed = match text.strip_prefix("0x") {
        Some(hex) => u16::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| format!("`{text}` is not a number"))
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Config, String> {
    let mut config = Config {
        device: "/dev/weft-sim".to_owned(),
        chips: Vec::new(),
        inputs: Vec::new(),
        steps: Vec::new(),
        trace: false,
    };
    let mut args = args.skip(1);
    match args.next().as_deref() {
        Some("serve") => {}
        _ => return Err(USAGE.to_owned()),
    }
    while let Some(flag) = args.next() {
        if flag == "--trace" {
            config.trace = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        let split = |sep| {
            value
                .split_once(sep)
                .ok_or_else(|| format!("{flag} {value}: expected `{sep}`"))
        };
        match flag.as_str() {
            "--device" => config.device.clone_from(&value),
            "--overlay" => config.steps.push(Step::Overlay(value.clone())),
            "--i2c" => {
                let (nr, path) = split('=')?;
                config
                    .steps
                    .push(Step::I2c(u32::from(number(nr)?), path.to_owned()));
            }
            "--chip" => {
                let (at, chip) = split('=')?;
                let (bus, addr) = at
                    .split_once(':')
                    .ok_or_else(|| format!("{flag} {value}: expected BUS:ADDR"))?;
                config
                    .chips
                    .push((number(bus)?, number(addr)?, chip.to_owned()));
            }
            "--input" => {
                let (at, volts) = split('=')?;
                let parts: Vec<&str> = at.split(':').collect();
                let [bus, addr, channel] = parts.as_slice() else {
                    return Err(format!("{flag} {value}: expected BUS:ADDR:CHANNEL=VOLTS"));
                };
                let volts = volts
                    .parse()
                    .map_err(|_| format!("`{volts}` is not a voltage"))?;
                config.inputs.push((
                    number(bus)?,
                    number(addr)?,
                    usize::from(number(channel)?),
                    volts,
                ));
            }
            _ => return Err(format!("unknown option {flag}\n{USAGE}")),
        }
    }
    Ok(config)
}

/// Builds the buses: each chip is looked up by name in the built-in weft-parts library.
///
/// The models take their time from the guest's monotonic clock. Under KVM, HVF or WHPX guest time
/// follows real time, so chips take as long as on hardware (an EEPROM write cycle, an ADC
/// conversion), but runs are not repeatable to the nanosecond. All buses share one clock.
fn build_buses(config: &Config) -> Result<BTreeMap<u16, I2cBus>, String> {
    let library = Library::builtin().map_err(|e| e.to_string())?;
    let clock = HostClock::new();
    let mut buses: BTreeMap<u16, I2cBus> = BTreeMap::new();
    for (bus, addr, name) in &config.chips {
        let suffix = format!("/{name}.toml");
        let chip = library
            .chips()
            .find(|(origin, _)| origin.ends_with(&suffix))
            .map(|(_, chip)| chip)
            .ok_or_else(|| format!("no chip `{name}` in weft-parts"))?;
        let model = chip
            .sim
            .as_ref()
            .ok_or_else(|| format!("chip `{name}` has no model"))?;
        let target = sim::build(model)?;
        buses
            .entry(*bus)
            .or_insert_with(|| I2cBus::with_clock(clock))
            .attach(*addr, target)
            .map_err(|_| format!("bus {bus}: address {addr:#04x} is used twice"))?;
    }
    for (bus, addr, channel, volts) in &config.inputs {
        let adc = buses
            .get_mut(bus)
            .and_then(|b| b.target_mut::<Ads1115>(*addr))
            .ok_or_else(|| format!("--input: no ADS1115 at bus {bus} address {addr:#04x}"))?;
        adc.set_input(*channel, *volts);
    }
    Ok(buses)
}

/// Answers requests until the device is closed or fails.
fn serve(device: &File, buses: &Mutex<BTreeMap<u16, I2cBus>>, trace: bool) -> std::io::Error {
    let mut buf = vec![0u8; device::MAX_XFER];
    let mut reader = device;
    let mut writer = device;
    loop {
        let len = match reader.read(&mut buf) {
            Ok(len) => len,
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {
                continue;
            }
            Err(e) => return e,
        };
        let raw = buf.get(..len).unwrap_or_default();
        let reply = match wire::Request::parse(raw) {
            Ok(request) => {
                let reply = match buses.lock() {
                    Ok(mut buses) => wire::answer(&request, &mut buses),
                    Err(_) => return std::io::Error::other("model state poisoned"),
                };
                if trace {
                    eprintln!("weft-sim: {request:?} -> {reply:02x?}");
                }
                reply
            }
            Err(wire::Malformed) => wire::malformed_reply(),
        };
        match writer.write(&reply) {
            Ok(_) => {}
            // The transfer timed out in the kernel before the reply arrived.
            Err(e) if e.raw_os_error() == Some(libc::ESTALE) => {
                eprintln!("weft-sim: late reply dropped");
            }
            Err(e) => return e,
        }
    }
}

fn run() -> Result<(), String> {
    let config = parse_args(std::env::args())?;
    let buses = Arc::new(Mutex::new(build_buses(&config)?));
    let device = File::options()
        .read(true)
        .write(true)
        .open(&config.device)
        .map(Arc::new)
        .map_err(|e| format!("{}: {e}", config.device))?;

    let server = {
        let (device, buses, trace) = (Arc::clone(&device), Arc::clone(&buses), config.trace);
        std::thread::spawn(move || serve(&device, &buses, trace))
    };

    for step in &config.steps {
        match step {
            Step::Overlay(path) => {
                let fdt = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
                device::apply_overlay(&device, &fdt).map_err(|e| format!("overlay {path}: {e}"))?;
            }
            Step::I2c(nr, path) => {
                device::add_i2c(&device, *nr, path)
                    .map_err(|e| format!("i2c {nr} at {path}: {e}"))?;
            }
        }
    }
    println!("weft-sim: ready");
    std::io::stdout().flush().map_err(|e| e.to_string())?;

    match server.join() {
        Ok(e) => Err(format!("serving stopped: {e}")),
        Err(_) => Err("serving thread failed".to_owned()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("weft-sim: {e}");
            ExitCode::FAILURE
        }
    }
}
