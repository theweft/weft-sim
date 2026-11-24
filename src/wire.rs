// SPDX-FileCopyrightText: 2026 The weft authors
// SPDX-License-Identifier: MIT

//! The request/reply format of `/dev/weft-sim` (see `module/weft_sim.h`) and how a request is
//! answered from simulated buses.

use std::collections::BTreeMap;

use weft_parts::sim::{I2cBus, Message, Nack};

/// `I2C_M_RD` from `<linux/i2c.h>`.
const I2C_M_RD: u16 = 0x0001;
/// `ENXIO`: nobody acknowledged. Linux I2C drivers treat it as "no device".
const ENXIO: i32 = 6;
/// `EPROTO`: the request could not be understood.
const EPROTO: i32 = 71;

const REQ_HEADER: usize = 8;
const MSG_DESC: usize = 8;

/// One message of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    /// Target address.
    pub addr: u16,
    /// Read (`true`) or write.
    pub read: bool,
    /// Bytes to read, or the bytes written.
    pub len: usize,
    /// Written bytes (empty for reads).
    pub data: Vec<u8>,
}

/// One transfer request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Sequence number, echoed in the reply.
    pub seq: u32,
    /// Adapter number.
    pub bus: u16,
    /// The messages, in order.
    pub msgs: Vec<Msg>,
}

/// A malformed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

fn u16_at(buf: &[u8], at: usize) -> Result<u16, Malformed> {
    let bytes = buf.get(at..at + 2).ok_or(Malformed)?;
    Ok(u16::from_ne_bytes(bytes.try_into().map_err(|_| Malformed)?))
}

fn u32_at(buf: &[u8], at: usize) -> Result<u32, Malformed> {
    let bytes = buf.get(at..at + 4).ok_or(Malformed)?;
    Ok(u32::from_ne_bytes(bytes.try_into().map_err(|_| Malformed)?))
}

impl Request {
    /// Parses a request as read from the device.
    ///
    /// # Errors
    /// When the buffer is shorter than its own headers say.
    pub fn parse(buf: &[u8]) -> Result<Self, Malformed> {
        let seq = u32_at(buf, 0)?;
        let bus = u16_at(buf, 4)?;
        let nmsgs = usize::from(u16_at(buf, 6)?);
        let mut data_at = REQ_HEADER + nmsgs * MSG_DESC;
        let mut msgs = Vec::with_capacity(nmsgs);
        for i in 0..nmsgs {
            let at = REQ_HEADER + i * MSG_DESC;
            let addr = u16_at(buf, at)?;
            let read = u16_at(buf, at + 2)? & I2C_M_RD != 0;
            let len = usize::from(u16_at(buf, at + 4)?);
            let data = if read {
                Vec::new()
            } else {
                let bytes = buf.get(data_at..data_at + len).ok_or(Malformed)?;
                data_at += len;
                bytes.to_vec()
            };
            msgs.push(Msg {
                addr,
                read,
                len,
                data,
            });
        }
        Ok(Self { seq, bus, msgs })
    }
}

/// The reply to a request that could not be parsed far enough to know its sequence number.
pub fn malformed_reply() -> Vec<u8> {
    reply(0, -EPROTO, &[])
}

fn reply(seq: u32, status: i32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + data.len());
    out.extend_from_slice(&seq.to_ne_bytes());
    out.extend_from_slice(&status.to_ne_bytes());
    out.extend_from_slice(data);
    out
}

/// Runs the request on the matching bus and builds the reply.
///
/// Messages to the same address run as one transfer with repeated starts, as on hardware;
/// a change of address starts a new transfer.
pub fn answer(request: &Request, buses: &mut BTreeMap<u16, I2cBus>) -> Vec<u8> {
    let Some(bus) = buses.get_mut(&request.bus) else {
        return reply(request.seq, -ENXIO, &[]);
    };
    let mut read_data = Vec::new();
    let mut start = 0;
    while start < request.msgs.len() {
        let addr = request.msgs.get(start).map_or(0, |m| m.addr);
        let end = request
            .msgs
            .iter()
            .skip(start)
            .position(|m| m.addr != addr)
            .map_or(request.msgs.len(), |n| start + n);
        let group = request.msgs.get(start..end).unwrap_or_default();
        match run_group(bus, addr, group) {
            Ok(bytes) => read_data.extend(bytes),
            Err(Nack) => return reply(request.seq, -ENXIO, &[]),
        }
        start = end;
    }
    reply(request.seq, 0, &read_data)
}

fn run_group(bus: &mut I2cBus, addr: u16, group: &[Msg]) -> Result<Vec<u8>, Nack> {
    let mut buffers: Vec<Vec<u8>> = group
        .iter()
        .filter(|m| m.read)
        .map(|m| vec![0; m.len])
        .collect();
    {
        let mut reads = buffers.iter_mut();
        let mut messages: Vec<Message<'_>> = Vec::with_capacity(group.len());
        for m in group {
            if m.read {
                let buffer = reads.next().ok_or(Nack)?;
                messages.push(Message::Read(buffer));
            } else {
                messages.push(Message::Write(&m.data));
            }
        }
        bus.transfer(addr, &mut messages)?;
    }
    Ok(buffers.concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use weft_parts::sim::{At24, VirtualClock};

    fn request(seq: u32, bus: u16, msgs: &[(u16, bool, &[u8], u16)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&seq.to_ne_bytes());
        out.extend_from_slice(&bus.to_ne_bytes());
        out.extend_from_slice(&u16::try_from(msgs.len()).unwrap_or(0).to_ne_bytes());
        for (addr, read, data, len) in msgs {
            let len = if *read {
                *len
            } else {
                u16::try_from(data.len()).unwrap_or(0)
            };
            out.extend_from_slice(&addr.to_ne_bytes());
            out.extend_from_slice(&u16::from(*read).to_ne_bytes());
            out.extend_from_slice(&len.to_ne_bytes());
            out.extend_from_slice(&0u16.to_ne_bytes());
        }
        for (_, read, data, _) in msgs {
            if !read {
                out.extend_from_slice(data);
            }
        }
        out
    }

    fn buses() -> BTreeMap<u16, I2cBus> {
        buses_with_clock(VirtualClock::new())
    }

    fn buses_with_clock(clock: VirtualClock) -> BTreeMap<u16, I2cBus> {
        let mut bus = I2cBus::with_clock(clock);
        let eeprom = At24::new(4096, 32).map(Box::new);
        assert!(eeprom.is_some());
        if let Some(eeprom) = eeprom {
            assert!(bus.attach(0x50, eeprom).is_ok());
        }
        BTreeMap::from([(1, bus)])
    }

    #[test]
    fn parses_messages_and_data() {
        let raw = request(7, 1, &[(0x50, false, &[0, 0x10], 0), (0x50, true, &[], 3)]);
        let parsed = Request::parse(&raw);
        assert_eq!(
            parsed,
            Ok(Request {
                seq: 7,
                bus: 1,
                msgs: vec![
                    Msg {
                        addr: 0x50,
                        read: false,
                        len: 2,
                        data: vec![0, 0x10]
                    },
                    Msg {
                        addr: 0x50,
                        read: true,
                        len: 3,
                        data: vec![]
                    },
                ],
            })
        );
    }

    #[test]
    fn truncated_request_is_malformed() {
        let raw = request(7, 1, &[(0x50, false, &[0, 0x10], 0)]);
        assert_eq!(
            Request::parse(raw.get(..raw.len() - 1).unwrap_or_default()),
            Err(Malformed)
        );
    }

    #[test]
    fn write_then_read_back_after_the_write_cycle() {
        let clock = VirtualClock::new();
        let mut buses = buses_with_clock(clock.clone());
        let write = Request::parse(&request(1, 1, &[(0x50, false, &[0, 0x10, 0xab, 0xcd], 0)]));
        assert!(write.is_ok());
        if let Ok(write) = write {
            assert_eq!(answer(&write, &mut buses), reply(1, 0, &[]));
        }
        let read = Request::parse(&request(
            2,
            1,
            &[(0x50, false, &[0, 0x10], 0), (0x50, true, &[], 2)],
        ));
        assert!(read.is_ok());
        if let Ok(read) = read {
            // During t_WR the EEPROM does not acknowledge: the Linux at24 driver retries on ENXIO.
            assert_eq!(answer(&read, &mut buses), reply(2, -ENXIO, &[]));
            clock.advance(Duration::from_millis(5));
            assert_eq!(answer(&read, &mut buses), reply(2, 0, &[0xab, 0xcd]));
        }
    }

    #[test]
    fn unknown_address_or_bus_is_enxio() {
        let mut buses = buses();
        for raw in [
            request(3, 1, &[(0x51, true, &[], 1)]),
            request(4, 2, &[(0x50, true, &[], 1)]),
        ] {
            let parsed = Request::parse(&raw);
            assert!(parsed.is_ok());
            if let Ok(parsed) = parsed {
                assert_eq!(answer(&parsed, &mut buses), reply(parsed.seq, -ENXIO, &[]));
            }
        }
    }
}
