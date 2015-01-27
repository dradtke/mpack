//! Experimental new MessagePack implementation for Rust.
//!
//! This crate provides an alternative MessagePack implementation
//! to [mneumann/rust-msgpack](http://github.com/mneumann/rust-msgpack).
//! The primary motivation for rewriting the crate is its current
//! broken build status coupled with some difficulty refactoring
//! away from Rust's old serialization story. This implementation
//! foregoes usage of `rustc-serialize` (at least until a new approach
//! stabilizes) in favor of operating directly on `Reader` and `Writer`.
//!
//! One thing to note is that the `Value::Extended` type is
//! currently just a placeholder, and is not yet implemented.
//! Only MessagePack's builtin formats as defined in the
//! [spec](https://github.com/msgpack/msgpack/blob/master/spec.md)
//! are implemented, but this will hopefully change soon.

#![crate_type = "lib"]
#![feature(unsafe_destructor)]
#![allow(dead_code, unstable)]

use std::error::{Error, FromError};
use std::io::{IoError, IoResult};
use std::string;

mod byte;
mod rpc;

macro_rules! nth_byte(
    ($x:expr, $n:expr) => ((($x >> ($n * 8)) & 0xFF) as u8)
);

/// A value that can be sent by `msgpack`.
#[derive(Clone, PartialEq, Show)]
#[stable]
pub enum Value {
    #[stable] Nil,
    #[stable] Boolean(bool),
    #[stable] Uint8(u8),
    #[stable] Uint16(u16),
    #[stable] Uint32(u32),
    #[stable] Uint64(u64),
    #[stable] Int8(i8),
    #[stable] Int16(i16),
    #[stable] Int32(i32),
    #[stable] Int64(i64),
    #[stable] Float32(f32),
    #[stable] Float64(f64),
    #[stable] String(string::String),
    #[stable] Binary(Vec<u8>),
    #[stable] Array(Vec<Value>),
    #[stable] Map(Vec<(Value, Value)>),

    // Still need to implement.
    #[unstable] Extended(i8, Vec<u8>),
}

impl Value {
    fn int(self) -> i64 {
        use Value::*;
        match self {
            Int8(i) => i as i64,
            Int16(i) => i as i64,
            Int32(i) => i as i64,
            Int64(i) => i,
            x => panic!("'{:?}' does not contain an int value", x),
        }
    }

    fn uint(self) -> u64 {
        use Value::*;
        match self {
            Uint8(i) => i as u64,
            Uint16(i) => i as u64,
            Uint32(i) => i as u64,
            Uint64(i) => i,
            x => panic!("'{:?}' does not contain a uint value", x),
        }
    }

    fn string(self) -> string::String {
        match self {
            Value::String(s) => s,
            _ => panic!("not a string value"),
        }
    }

    fn array(self) -> Vec<Value> {
        match self {
            Value::Array(ar) => ar,
            _ => panic!("not an array value"),
        }
    }
}

/// A trait for types that can be written via MessagePack. This is mostly
/// a convenience to avoid having to wrap them yourself each time.
#[stable]
pub trait IntoValue {
    fn into_value(self) -> Value;
}

impl IntoValue for () { fn into_value(self) -> Value { Value::Nil } }
impl IntoValue for bool { fn into_value(self) -> Value { Value::Boolean(self) } }
impl IntoValue for u8 { fn into_value(self) -> Value { Value::Uint8(self) } }
impl IntoValue for u16 { fn into_value(self) -> Value { Value::Uint16(self) } }
impl IntoValue for u32 { fn into_value(self) -> Value { Value::Uint32(self) } }
impl IntoValue for u64 { fn into_value(self) -> Value { Value::Uint64(self) } }
impl IntoValue for i8 { fn into_value(self) -> Value { Value::Int8(self) } }
impl IntoValue for i16 { fn into_value(self) -> Value { Value::Int16(self) } }
impl IntoValue for i32 { fn into_value(self) -> Value { Value::Int32(self) } }
impl IntoValue for i64 { fn into_value(self) -> Value { Value::Int64(self) } }
impl IntoValue for f32 { fn into_value(self) -> Value { Value::Float32(self) } }
impl IntoValue for f64 { fn into_value(self) -> Value { Value::Float64(self) } }
impl IntoValue for string::String { fn into_value(self) -> Value { Value::String(self) } }

// TODO: re-enable this when we can specify that the implementation
// for Vec<T> should *not* include u8
/*
impl IntoValue for Vec<u8> {
    fn into_value(self) -> Value { Value::Binary(self) }
}

impl IntoValue for &'static [u8] {
    fn into_value(self) -> Value {
        let mut ar = Vec::with_capacity(self.len());
        ar.push_all(self);
        Value::Binary(ar)
    }
}
*/

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::Array(self.into_iter().map(|v| v.into_value()).collect())
    }
}

// TODO: try and get this to work
/*
impl<T: IntoValue, V: IntoValue> IntoValue for std::collections::HashMap<T, V> {
    fn into_value(self) -> Value {
        Value::Map(self.into_iter().map(|(k, v)| (k.into_value(), v.pack())).collect())
    }
}
*/

/// Convenience wrapper for `write_value()`.
#[unstable = "exact API may change"]
pub fn write<W: Writer, V: IntoValue>(dest: &mut W, val: V) -> IoResult<()> {
    write_value(dest, val.into_value())
}

/// Write any value as an Extended type.
///
/// The `val` parameter will be automatically converted to its byte representation.
#[unstable = "exact API may change"]
pub fn write_ext<W: Writer, T>(dest: &mut W, id: i8, val: T) -> IoResult<()> {
    let data: &[u8] = unsafe {
        std::mem::transmute(std::raw::Slice {
            data: &val as *const _ as *const u8,
            len: std::mem::size_of::<T>(),
        })
    };
    write_value(dest, Value::Extended(id, data.to_vec()))
}

/// Write a message in MessagePack format for the given value.
#[unstable = "exact API may change"]
pub fn write_value<W: Writer>(dest: &mut W, val: Value) -> IoResult<()> {
    use Value::*;

    match val {
        Nil => dest.write_u8(byte::NIL),

        Boolean(false) => dest.write_u8(byte::FALSE),
        Boolean(true) => dest.write_u8(byte::TRUE),

        Uint8(x) => dest.write(&[byte::U8, x]),
        Uint16(x) => { try!(dest.write_u8(byte::U16)); try!(dest.write_be_u16(x)); Ok(()) },
        Uint32(x) => { try!(dest.write_u8(byte::U32)); try!(dest.write_be_u32(x)); Ok(()) },
        Uint64(x) => { try!(dest.write_u8(byte::U64)); try!(dest.write_be_u64(x)); Ok(()) },

        Int8(x) => { try!(dest.write_u8(byte::I8)); try!(dest.write_i8(x)); Ok(()) },
        Int16(x) => { try!(dest.write_u8(byte::I16)); try!(dest.write_be_i16(x)); Ok(()) },
        Int32(x) => { try!(dest.write_u8(byte::I32)); try!(dest.write_be_i32(x)); Ok(()) },
        Int64(x) => { try!(dest.write_u8(byte::I64)); try!(dest.write_be_i64(x)); Ok(()) },

        Float32(x) => { try!(dest.write_u8(byte::F32)); try!(dest.write_be_f32(x)); Ok(()) },
        Float64(x) => { try!(dest.write_u8(byte::F64)); try!(dest.write_be_f64(x)); Ok(()) },

        String(s) => {
            let bytes = s.as_bytes();
            let n = bytes.len();
            match n {
                0...31 => { // fixstr
                    try!(dest.write_u8((0b10100000 | n) as u8));
                },
                32...255 => { // str 8
                    try!(dest.write(&[byte::STR8, n as u8]));
                },
                256...65535 => { // str 16
                    try!(dest.write_u8(byte::STR16));
                    try!(dest.write_be_u16(n as u16));
                },
                65536...4294967295 => { // str 32
                    try!(dest.write_u8(byte::STR32));
                    try!(dest.write_be_u32(n as u32));
                },
                _ => panic!("string too long! {} bytes is too many!", n),
            }
            try!(dest.write(bytes));
            Ok(())
        },

        Binary(b) => {
            let n = b.len();
            match n {
                0...255 => { // bin 8
                    try!(dest.write(&[byte::BIN8, n as u8]));
                },
                256...65535 => { // bin 16
                    try!(dest.write_u8(byte::BIN16));
                    try!(dest.write_be_u16(n as u16));
                },
                65536...4294967295 => { // bin 32
                    try!(dest.write_u8(byte::BIN32));
                    try!(dest.write_be_u32(n as u32));
                },
                // TODO: encode the other lengths
                _ => panic!("binary data too long! {} bytes is too many!", n),
            }
            try!(dest.write(b.as_slice()));
            Ok(())
        },

        Array(values) => {
            let n = values.len();
            match n {
                0...15 => { // fixarray
                    try!(dest.write_u8((0b10010000 | n) as u8));
                },
                16...65535 => { // 16 array
                    try!(dest.write_u8(byte::AR16));
                    try!(dest.write_be_u16(n as u16));
                },
                65536...4294967295 => { // 32 array
                    try!(dest.write_u8(byte::AR32));
                    try!(dest.write_be_u32(n as u32));
                },
                _ => panic!("array too long! {} bytes is too many!", n),
            }
            for v in values.into_iter() {
                try!(write_value(dest, v));
            }
            Ok(())
        },

        Map(entries) => {
            let n = entries.len();
            match n {
                0...15 => { // fixmap
                    try!(dest.write_u8((0b10000000 | n) as u8));
                },
                16...65535 => { // 16 map
                    try!(dest.write_u8(byte::MAP16));
                    try!(dest.write_be_u16(n as u16));
                },
                65536...4294967295 => { // 32 map
                    try!(dest.write_u8(byte::MAP32));
                    try!(dest.write_be_u32(n as u32));
                },
                _ => panic!("map too long! {} bytes is too many!", n),
            }
            for (k, v) in entries.into_iter() {
                try!(write_value(dest, k));
                try!(write_value(dest, v));
            }
            Ok(())
        },

        Extended(id, data) => {
            let n = data.len();
            try!(match n {
                1 => { dest.write_u8(byte::FIXEXT1) },
                2 => { dest.write_u8(byte::FIXEXT2) },
                4 => { dest.write_u8(byte::FIXEXT4) },
                8 => { dest.write_u8(byte::FIXEXT8) },
                16 => { dest.write_u8(byte::FIXEXT16) },

                0...255 => { // ext 8
                    dest.write(&[byte::EXT8, n as u8])
                },
                256...65535 => { // ext 16
                    try!(dest.write_u8(byte::EXT16));
                    dest.write_be_u16(n as u16)
                },
                65536...4294967295 => { // ext 32
                    try!(dest.write_u8(byte::EXT32));
                    dest.write_be_u32(n as u32)
                },

                _ => panic!("custom value too long! {} bytes is too many!", n),
            });
            try!(dest.write_i8(id));
            dest.write(data.as_slice())
        },
    }
}

/// Read a MessagePack message from the given `Reader`.
///
/// Ideally this method would just peek at the first byte
/// to see if it recognizes it as a format id, but plain
/// Readers don't support that, so for now we just consume
/// it and return the unrecognized byte as part of the error.
#[unstable = "the exact signature may change"]
pub fn read_value<R: Reader>(src: &mut R) -> Result<Value, ReadError> {
    use Value::*;

    match try!(src.read_byte()) {
        byte::NIL => Ok(Nil),

        byte::FALSE => Ok(Boolean(false)),
        byte::TRUE => Ok(Boolean(true)),

        byte::U8 => Ok(Uint8(try!(src.read_u8()))),
        byte::U16 => Ok(Uint16(try!(src.read_be_u16()))),
        byte::U32 => Ok(Uint32(try!(src.read_be_u32()))),
        byte::U64 => Ok(Uint64(try!(src.read_be_u64()))),

        byte::I8 => Ok(Int8(try!(src.read_i8()))),
        byte::I16 => Ok(Int16(try!(src.read_be_i16()))),
        byte::I32 => Ok(Int32(try!(src.read_be_i32()))),
        byte::I64 => Ok(Int64(try!(src.read_be_i64()))),

        byte::F32 => Ok(Float32(try!(src.read_be_f32()))),
        byte::F64 => Ok(Float64(try!(src.read_be_f64()))),

        b if (b >> 5) == 0b00000101 => {
            let n = (b & 0b00011111) as usize;
            let bytes = try!(src.read_exact(n));
            match string::String::from_utf8(bytes) {
                Ok(s) => Ok(String(s)),
                Err(_) => panic!("received invalid utf-8"),
            }
        },

        byte::STR8 => {
            let n = try!(src.read_byte()) as usize;
            let bytes = try!(src.read_exact(n));
            match string::String::from_utf8(bytes) {
                Ok(s) => Ok(String(s)),
                Err(_) => panic!("received invalid utf-8"),
            }
        },

        byte::STR16 => {
            let n = try!(src.read_be_u16()) as usize;
            let bytes = try!(src.read_exact(n));
            match string::String::from_utf8(bytes) {
                Ok(s) => Ok(String(s)),
                Err(_) => panic!("received invalid utf-8"),
            }
        },

        byte::STR32 => {
            let n = try!(src.read_be_u32()) as usize;
            let bytes = try!(src.read_exact(n));
            match string::String::from_utf8(bytes) {
                Ok(s) => Ok(String(s)),
                Err(_) => panic!("received invalid utf-8"),
            }
        },

        byte::BIN8 => {
            let n = try!(src.read_byte()) as usize;
            Ok(Binary(try!(src.read_exact(n))))
        },

        byte::BIN16 => {
            let n = try!(src.read_be_u16()) as usize;
            Ok(Binary(try!(src.read_exact(n))))
        },

        byte::BIN32 => {
            let n = try!(src.read_be_u32()) as usize;
            Ok(Binary(try!(src.read_exact(n))))
        },

        b if (b >> 4) == 0b00001001 => {
            let n = (b & 0b00001111) as usize;
            let mut ar = Vec::with_capacity(n);
            for _ in range(0, n) {
                ar.push(try!(read_value(src)));
            }
            Ok(Array(ar))
        },

        byte::AR16 => {
            let n = try!(src.read_be_u16()) as usize;
            let mut ar = Vec::with_capacity(n);
            for _ in range(0, n) {
                ar.push(try!(read_value(src)));
            }
            Ok(Array(ar))
        },

        byte::AR32 => {
            let n = try!(src.read_be_u32()) as usize;
            let mut ar = Vec::with_capacity(n);
            for _ in range(0, n) {
                ar.push(try!(read_value(src)));
            }
            Ok(Array(ar))
        },

        b if (b >> 4) == 0b00001000 => {
            let n = (b & 0b00001111) as usize;
            let mut m = Vec::with_capacity(n);
            for _ in range(0, n) {
                m.push((try!(read_value(src)), try!(read_value(src))));
            }
            Ok(Map(m))
        },

        byte::MAP16 => {
            let n = try!(src.read_be_u16()) as usize;
            let mut m = Vec::with_capacity(n);
            for _ in range(0, n) {
                m.push((try!(read_value(src)), try!(read_value(src))));
            }
            Ok(Map(m))
        },

        byte::MAP32 => {
            let n = try!(src.read_be_u32()) as usize;
            let mut m = Vec::with_capacity(n);
            for _ in range(0, n) {
                m.push((try!(read_value(src)), try!(read_value(src))));
            }
            Ok(Map(m))
        },

        // Extension types.
        byte::FIXEXT1 => Ok(Extended(try!(src.read_i8()), try!(src.read_exact(1)))),
        byte::FIXEXT2 => Ok(Extended(try!(src.read_i8()), try!(src.read_exact(2)))),
        byte::FIXEXT4 => Ok(Extended(try!(src.read_i8()), try!(src.read_exact(4)))),
        byte::FIXEXT8 => Ok(Extended(try!(src.read_i8()), try!(src.read_exact(8)))),
        byte::FIXEXT16 => Ok(Extended(try!(src.read_i8()), try!(src.read_exact(16)))),

        byte::EXT8 => {
            let n = try!(src.read_u8()) as usize;
            Ok(Extended(try!(src.read_i8()), try!(src.read_exact(n))))
        },

        byte::EXT16 => {
            let n = try!(src.read_be_u16()) as usize;
            Ok(Extended(try!(src.read_i8()), try!(src.read_exact(n))))
        },

        byte::EXT32 => {
            let n = try!(src.read_be_u32()) as usize;
            Ok(Extended(try!(src.read_i8()), try!(src.read_exact(n))))
        },

        x => Err(ReadError::Unrecognized(x)),
    }
}

/* -- Errors -- */

/// An error encountered while trying to read a value.
#[stable]
#[derive(Show)]
pub enum ReadError {
    #[stable] Io(IoError),
    #[stable] Unrecognized(u8),
}

impl Error for ReadError {
    fn description(&self) -> &str { "read error" }
    fn cause(&self) -> Option<&Error> {
        match *self {
            ReadError::Io(ref e) => Some(e as &Error),
            ReadError::Unrecognized(_) => None,
        }
    }
}

// For some reason the Error implementation barks if you don't do this.
impl std::fmt::Display for ReadError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(fmt, "{}", self)
    }
}

impl FromError<IoError> for ReadError {
    fn from_error(e: IoError) -> ReadError { ReadError::Io(e) }
}

#[cfg(test)]
mod test {
    use std::io::{ChanReader, ChanWriter};
    use std::sync::mpsc::channel;
    use std::rand::{Rng, StdRng};
    use std::string;
    use super::{IntoValue, read_value, write_value};

    const LETTERS: &'static [char] = &['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z'];

    fn test<T: IntoValue>(arg: T) {
        let val = arg.into_value();
        let (tx, rx) = channel();
        write_value(&mut ChanWriter::new(tx), val.clone()).unwrap();
        assert_eq!(read_value(&mut ChanReader::new(rx)).unwrap(), val);
    }

    fn random_string(n: usize) -> string::String {
        let mut rng = StdRng::new().unwrap();

        let mut s = string::String::with_capacity(n);
        for _ in range(0, n) {
            s.push(*rng.choose(LETTERS).unwrap());
        }
        s
    }

    #[test] fn test_nil() { test(()); }

    #[test] fn test_u8() { test(3 as u8); }
    #[test] fn test_u16() { test(36 as u16); }
    #[test] fn test_u32() { test(360 as u32); }
    #[test] fn test_u64() { test(3600 as u64); }
    #[test] fn test_i8() { test(3 as i8); }
    #[test] fn test_i16() { test(36 as i16); }
    #[test] fn test_i32() { test(360 as i32); }
    #[test] fn test_i64() { test(3600 as i64); }

    #[test] fn test_f32() { test(1234.56 as f32); }
    #[test] fn test_f64() { test(123456.78 as f64); }

    #[test] fn write_tiny_string() { test(random_string(8)); }
    #[test] fn write_short_string() { test(random_string(32)); }
    #[test] fn write_medium_string() { test(random_string(256)); }
}
