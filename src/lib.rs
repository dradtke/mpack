//! Experimental new MessagePack implementation for Rust.
//!
//! This crate provides an alternative MessagePack implementation
//! to [mneumann/rust-msgpack](http://github.com/mneumann/rust-msgpack).
//! This implementation foregoes usage of `rustc-serialize` (at least
//! until a new approach stabilizes) in favor of operating directly on
//! `std::io::Read` and `std::io::Write`.
//!
//! Better serialization sugar is planned, but for now, here's how you
//! would open up a connection and send a value to it:
//!
//! ~~~
//! use std::net::TcpStream;
//! use mpack::{Value, write_value};
//!
//! let mut conn = TcpStream::connect("127.0.0.1:8081").unwrap();
//!
//! // write some values
//! write_value(&mut conn, Value::Int32(3)).unwrap();
//! write_value(&mut conn, Value::String("hello world".to_string())).unwrap();
//! ~~~
//!
//! Reading values is just as easy:
//!
//! ~~~
//! use std::net::TcpStream;
//! use mpack::{Value, read_value};
//!
//! let mut conn = TcpStream::connect("127.0.0.1:8081").unwrap();
//!
//! match read_value(&mut conn).unwrap() {
//!     Value::Int32(x) => assert_eq!(x, 3),
//!     _ => panic!("received unexpected value"),
//! }
//!
//! match read_value(&mut conn).unwrap() {
//!     Value::String(str) => println!("{}", str),
//!     _ => panic!("received unexpected value"),
//! }
//! ~~~

#![crate_type = "lib"]
#![feature(collections, core, io, std_misc, unsafe_destructor)]
#![allow(dead_code)]

use std::any::{TypeId};
//use std::collections::hash_map::HashMap;
use std::error::{Error, FromError};
use std::io::{self, Read, Write};
use std::string;

mod byte;
pub mod rpc;

macro_rules! nth_byte(
    ($x:expr, $n:expr) => ((($x >> ($n * 8)) & 0xFF) as u8)
);

/// A value that can be sent by `msgpack`.
#[derive(Clone, PartialEq, Debug)]
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

    fn get<T: IntoValue>(&self, key: T) -> Option<&Value> {
        let key = key.into_value();
        match *self {
            Value::Map(ref map) => {
                map.iter().find(|&&(ref k, _)| *k == key).map(|&(_, ref v)| v)
            },
            _ => panic!("not a map value"),
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

/// Wraps a reader instance with a place to store the last byte that was read. This
/// simulates pushing that byte back on to the reader if it wasn't recognized.
pub struct Reader<R: Read + Send> {
    next_byte: Option<u8>,
    reader: R,
}

impl<R: Read + Send> Reader<R> {
    pub fn new(reader: R) -> Reader<R> {
        Reader{ next_byte: None, reader: reader }
    }

    pub fn read_value(&mut self) -> Result<Value, ReadError> {
        use Value::*;
        use std::io::ReadExt;

        // Get the next byte, using the cached value if there is one.
        let mut b = [0];
        match self.next_byte {
            Some(byte) => b[0] = byte,
            None => if try!(self.reader.read(&mut b)) == 0 {
                return Err(ReadError::NoData);
            },
        }

        self.next_byte = None;

        // Reads a big-endian integer from the provided reader. It's based on the implementation
        // of std::old_io::Reader::read_be_uint_n, but ends up being slightly more efficient because
        // it will store the result in the correctly-sized value rather than shove everything
        // into a u64.
        macro_rules! read_be(
            ($src:expr, $int:ident) => ({
                let mut val: $int = 0;
                for (i, next) in $src.by_ref().take(std::$int::BYTES as u64).bytes().enumerate() {
                    val += (try!(next) as $int) << (std::$int::BYTES - ((i + 1) as u32)) * 8;
                }
                Ok::<$int, ReadError>(val)
            })
        );

        // Reads an exact number of bytes from the provided reader.
        macro_rules! read_exact(
            ($src:expr, $n:expr) => ({
                let mut v = Vec::with_capacity($n);
                for next in $src.by_ref().take($n as u64).bytes() {
                    v.push(try!(next));
                }
                v
            })
        );

        match b[0] {
            byte::NIL => Ok(Nil),

            byte::FALSE => Ok(Boolean(false)),
            byte::TRUE => Ok(Boolean(true)),

            byte::U8 => Ok(Uint8(try!(read_be!(self.reader, u8)))),
            byte::U16 => Ok(Uint16(try!(read_be!(self.reader, u16)))),
            byte::U32 => Ok(Uint32(try!(read_be!(self.reader, u32)))),
            byte::U64 => Ok(Uint64(try!(read_be!(self.reader, u64)))),

            byte::I8 => Ok(Int8(try!(read_be!(self.reader, i8)))),
            byte::I16 => Ok(Int16(try!(read_be!(self.reader, i16)))),
            byte::I32 => Ok(Int32(try!(read_be!(self.reader, i32)))),
            byte::I64 => Ok(Int64(try!(read_be!(self.reader, i64)))),

            byte::F32 => Ok(Float32(try!(read_be!(self.reader, u32)) as f32)),
            byte::F64 => Ok(Float64(try!(read_be!(self.reader, u64)) as f64)),

            b if (b >> 5) == 0b00000101 => {
                let n = (b & 0b00011111) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::STR8 => {
                let n = try!(read_be!(self.reader, u8)) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::STR16 => {
                let n = try!(read_be!(self.reader, u16)) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::STR32 => {
                let n = try!(read_be!(self.reader, u32)) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::BIN8 => {
                let n = try!(read_be!(self.reader, u8)) as usize;
                Ok(Binary(read_exact!(self.reader, n)))
            },

            byte::BIN16 => {
                let n = try!(read_be!(self.reader, u16)) as usize;
                Ok(Binary(read_exact!(self.reader, n)))
            },

            byte::BIN32 => {
                let n = try!(read_be!(self.reader, u32)) as usize;
                Ok(Binary(read_exact!(self.reader, n)))
            },

            b if (b >> 4) == 0b00001001 => {
                let n = (b & 0b00001111) as usize;
                let mut ar = Vec::with_capacity(n);
                for _ in range(0, n) {
                    ar.push(try!(self.read_value()));
                }
                Ok(Array(ar))
            },

            byte::AR16 => {
                let n = try!(read_be!(self.reader, u16)) as usize;
                let mut ar = Vec::with_capacity(n);
                for _ in range(0, n) {
                    ar.push(try!(self.read_value()));
                }
                Ok(Array(ar))
            },

            byte::AR32 => {
                let n = try!(read_be!(self.reader, u32)) as usize;
                let mut ar = Vec::with_capacity(n);
                for _ in range(0, n) {
                    ar.push(try!(self.read_value()));
                }
                Ok(Array(ar))
            },

            b if (b >> 4) == 0b00001000 => {
                let n = (b & 0b00001111) as usize;
                let mut m = Vec::with_capacity(n);
                for _ in range(0, n) {
                    m.push((try!(self.read_value()), try!(self.read_value())));
                }
                Ok(Map(m))
            },

            byte::MAP16 => {
                let n = try!(read_be!(self.reader, u16)) as usize;
                let mut m = Vec::with_capacity(n);
                for _ in range(0, n) {
                    m.push((try!(self.read_value()), try!(self.read_value())));
                }
                Ok(Map(m))
            },

            byte::MAP32 => {
                let n = try!(read_be!(self.reader, u32)) as usize;
                let mut m = Vec::with_capacity(n);
                for _ in range(0, n) {
                    m.push((try!(self.read_value()), try!(self.read_value())));
                }
                Ok(Map(m))
            },

            // Extension types.
            byte::FIXEXT1 => Ok(Extended(try!(read_be!(self.reader, i8)), read_exact!(self.reader, 1))),
            byte::FIXEXT2 => Ok(Extended(try!(read_be!(self.reader, i8)), read_exact!(self.reader, 2))),
            byte::FIXEXT4 => Ok(Extended(try!(read_be!(self.reader, i8)), read_exact!(self.reader, 4))),
            byte::FIXEXT8 => Ok(Extended(try!(read_be!(self.reader, i8)), read_exact!(self.reader, 8))),
            byte::FIXEXT16 => Ok(Extended(try!(read_be!(self.reader, i8)), read_exact!(self.reader, 16))),

            byte::EXT8 => {
                let n = try!(read_be!(self.reader, u8)) as usize;
                Ok(Extended(try!(read_be!(self.reader, i8)), read_exact!(self.reader, n)))
            },

            byte::EXT16 => {
                let n = try!(read_be!(self.reader, u16)) as usize;
                Ok(Extended(try!(read_be!(self.reader, i8)), read_exact!(self.reader, n)))
            },

            byte::EXT32 => {
                let n = try!(read_be!(self.reader, u32)) as usize;
                Ok(Extended(try!(read_be!(self.reader, i8)), read_exact!(self.reader, n)))
            },

            x => {
                self.next_byte = Some(x);
                Err(ReadError::Unrecognized(x))
            },
        }
    }
}

/// Convenience wrapper for `write_value()`.
#[unstable = "exact API may change"]
pub fn write<W: Write, V: IntoValue>(dest: &mut W, val: V) -> Result<(), WriteError> {
    write_value(dest, val.into_value())
}

/// Write any value as an Extended type.
///
/// The `val` parameter will be automatically converted to its byte representation.
#[unstable = "exact API may change"]
pub fn write_ext<W: Write, T>(dest: &mut W, id: i8, val: T) -> Result<(), WriteError> {
    let data: &[u8] = unsafe {
        std::mem::transmute(std::raw::Slice {
            data: &val as *const _ as *const u8,
            len: std::mem::size_of::<T>(),
        })
    };
    write_value(dest, Value::Extended(id, data.to_vec()))
}

//pub fn build_ext<T>(data: Vec<u8>) -> T {
//    unsafe {
//        std::mem::transmute(data)
//    }
//}

/// Write a message in MessagePack format for the given value.
#[unstable = "exact API may change"]
pub fn write_value<W: Write>(dest: &mut W, val: Value) -> Result<(), WriteError> {
    use Value::*;
    use std::mem::transmute;

    // Convert a number to its big-endian byte representation.
    macro_rules! b(
        ($x:expr, $n:ident) => ({
            use std::num::Int;
            unsafe { transmute::<_, [u8; std::$n::BYTES as usize]>(($x as $n).to_be()) }
        })
    );

    // Create a slice out of a leading byte with any number of slices appended to it.
    macro_rules! data(
        ($b:expr; $($data:expr),+) => ({
            let mut v = Vec::with_capacity({ let mut size = 1; $( size += $data.len(); )+ size });
            v.push($b); $( v.push_all(&$data); )+ &v.into_boxed_slice()
        })
    );

    match val {
        Nil => try!(dest.write_all(&[byte::NIL])),

        Boolean(false) => try!(dest.write_all(&[byte::FALSE])),
        Boolean(true) => try!(dest.write_all(&[byte::TRUE])),

        Uint8(x) => try!(dest.write_all(&[byte::U8, x])),
        Uint16(x) => try!(dest.write_all(data![byte::U16; b!(x, u16)])),
        Uint32(x) => try!(dest.write_all(data![byte::U32; b!(x, u32)])),
        Uint64(x) => try!(dest.write_all(data![byte::U64; b!(x, u64)])),

        Int8(x) => try!(dest.write_all(&[byte::I8, x as u8])),
        Int16(x) => try!(dest.write_all(data![byte::I16; b!(x, i16)])),
        Int32(x) => try!(dest.write_all(data![byte::I32; b!(x, i32)])),
        Int64(x) => try!(dest.write_all(data![byte::I64; b!(x, i64)])),

        Float32(x) => try!(dest.write_all(data![byte::F32; b!(x, u32)])),
        Float64(x) => try!(dest.write_all(data![byte::F64; b!(x, u64)])),

        String(s) => {
            let bytes = s.as_bytes();
            let n = bytes.len();
            try!(match n {
                0...31 => Ok(try!(dest.write_all(&[(0b10100000 | n) as u8]))), // fixstr
                32...255 => Ok(try!(dest.write_all(&[byte::STR8, n as u8]))), // str 8
                256...65535 => Ok(try!(dest.write_all(data![byte::STR16; b!(n, u16), bytes]))), // str 16
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::STR32; b!(n, u32), bytes]))), // str 32
                _ => Err(WriteError::TooMuchData(n)),
            });
        },

        Binary(b) => {
            let n = b.len();
            try!(match n {
                0...255 => Ok(try!(dest.write_all(data![byte::BIN8; b!(n, u8), b.as_slice()]))), // bin 8
                256...65535 => Ok(try!(dest.write_all(data![byte::BIN16; b!(n, u16), b.as_slice()]))), // bin 16
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::BIN32; b!(n, u32), b.as_slice()]))), // bin 32
                _ => Err(WriteError::TooMuchData(n)),
            });
        },

        Array(values) => {
            let n = values.len();
            try!(match n {
                0...15 => Ok(try!(dest.write_all(&[(0b10010000 | n) as u8]))), // fixarray
                16...65535 => Ok(try!(dest.write_all(data![byte::AR16; b!(n, u16)]))), // 16 array
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::AR32; b!(n, u32)]))), // 32 array
                _ => Err(WriteError::TooMuchData(n)),
            });
            for v in values.into_iter() {
                try!(write_value(dest, v));
            }
        },

        Map(entries) => {
            let n = entries.len();
            try!(match n {
                0...15 => Ok(try!(dest.write_all(&[(0b10000000 | n) as u8]))), // fixmap
                16...65535 => Ok(try!(dest.write_all(data![byte::MAP16; b!(n, u16)]))), // 16 map
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::MAP32; b!(n, u32)]))), // 32 map
                _ => Err(WriteError::TooMuchData(n)),
            });
            for (k, v) in entries.into_iter() {
                try!(write_value(dest, k));
                try!(write_value(dest, v));
            }
        },

        Extended(id, data) => {
            let n = data.len();
            try!(match n {
                1 => Ok(try!(dest.write_all(&[byte::FIXEXT1]))),
                2 => Ok(try!(dest.write_all(&[byte::FIXEXT2]))),
                4 => Ok(try!(dest.write_all(&[byte::FIXEXT4]))),
                8 => Ok(try!(dest.write_all(&[byte::FIXEXT8]))),
                16 => Ok(try!(dest.write_all(&[byte::FIXEXT16]))),
                0...255 => Ok(try!(dest.write_all(&[byte::EXT8, n as u8]))), // ext 8
                256...65535 => Ok(try!(dest.write_all(data![byte::MAP16; b!(n, u16)]))), // ext 16
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::MAP32; b!(n, u32)]))), // ext 32
                _ => Err(WriteError::TooMuchData(n)),
            });
            try!(dest.write_all(&[id as u8]));
            try!(dest.write_all(data.as_slice()));
        },
    }
    Ok(())
}

/// Read a MessagePack message from the given `Read`.
///
/// Ideally this method would just peek at the first byte
/// to see if it recognizes it as a format id, but plain
/// Readers don't support that, so for now we just consume
/// it and return the unrecognized byte as part of the error.
#[unstable = "the exact signature may change"]

/* -- Errors -- */

/// An error encountered while trying to write a value.
#[derive(Debug)]
pub enum WriteError {
    Io(io::Error),
    TooMuchData(usize),
    UnregisteredExt(TypeId),
}

impl Error for WriteError {
    fn description(&self) -> &str { "write error" }
    fn cause(&self) -> Option<&Error> {
        match *self {
            WriteError::Io(ref e) => Some(e as &Error),
            WriteError::TooMuchData(..) => None,
            WriteError::UnregisteredExt(..) => None,
        }
    }
}

// For some reason the Error implementation barks if you don't do this.
impl std::fmt::Display for WriteError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(fmt, "{}", self)
    }
}

impl FromError<io::Error> for WriteError {
    fn from_error(e: io::Error) -> WriteError { WriteError::Io(e) }
}

/// An error encountered while trying to read a value.
#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    NoData,
    NotExtended,
    Unrecognized(u8),
}

impl Error for ReadError {
    fn description(&self) -> &str { "read error" }
    fn cause(&self) -> Option<&Error> {
        match *self {
            ReadError::Io(ref e) => Some(e as &Error),
            ReadError::NoData => None,
            ReadError::NotExtended => None,
            ReadError::Unrecognized(..) => None,
        }
    }
}

// For some reason the Error implementation barks if you don't do this.
impl std::fmt::Display for ReadError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(fmt, "{}", self)
    }
}

impl FromError<io::Error> for ReadError {
    fn from_error(e: io::Error) -> ReadError { ReadError::Io(e) }
}

#[cfg(test)]
mod test {
    extern crate rand;

    use std::old_io::{ChanReader, ChanWriter};
    use std::sync::mpsc::channel;
    use std::string;
    use self::rand::{Rng, StdRng};
    use super::{IntoValue, write_value};

    const LETTERS: &'static [char] = &['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z'];

    fn test<T: IntoValue>(arg: T) {
        let val = arg.into_value();
        let (tx, rx) = channel();
        write_value(&mut ChanWriter::new(tx), val.clone()).unwrap();
        assert_eq!(super::Reader::new(&mut ChanReader::new(rx)).read_value().unwrap(), val);
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
