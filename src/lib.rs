//! A MessagePack implementation for Rust.
//!
//! ~~~ignore
//! use std::net::TcpStream;
//! use mpack::{Value, write_value};
//!
//! let mut conn = TcpStream::connect("127.0.0.1:8081").unwrap();
//!
//! // write values
//! write(&mut conn, 3 as i32).unwrap();
//! ~~~
//!
//! Reading values is just as easy:
//!
//! ~~~ignore
//! use std::net::TcpStream;
//! use mpack::{Value, Reader};
//!
//! let mut conn = TcpStream::connect("127.0.0.1:8081").unwrap();
//! let mut reader = Reader::new(conn);
//!
//! let value = reader.read_value().unwrap();
//! // `value` can be inspected with `match` or converted directly with a convenience method
//! ~~~

#![crate_type = "lib"]
#![allow(dead_code)]

use std::any::TypeId;
use std::error::Error;
use std::slice;
use std::io::{self, Read, Write};
use std::string;

mod byte;
pub mod rpc;

macro_rules! nth_byte(
    ($x:expr, $n:expr) => ((($x >> ($n * 8)) & 0xFF) as u8)
);

/// A value that can be sent by `msgpack`.
#[derive(Clone, PartialEq, Debug)]
pub enum Value {
    Nil,
    Boolean(bool),
    Uint8(u8),
    Uint16(u16),
    Uint32(u32),
    Uint64(u64),
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Float32(f32),
    Float64(f64),
    String(string::String),
    Binary(Vec<u8>),
    Array(Vec<Value>),
    Map(ValueMap),
    Extended(i8, Vec<u8>),
}

impl Value {
    pub fn is_nil(&self) -> bool {
        match *self {
            Value::Nil => true,
            _ => false,
        }
    }

    pub fn bool(self) -> Result<bool, TypeError> {
        use Value::*;
        match self {
            Boolean(b) => Ok(b),
            v => Err(TypeError{desc: format!("{:?} does not contain a boolean value", v), v: v}),
        }
    }

    pub fn int(self) -> Result<i64, TypeError> {
        use Value::*;
        match self {
            Int8(i) => Ok(i as i64),
            Int16(i) => Ok(i as i64),
            Int32(i) => Ok(i as i64),
            Int64(i) => Ok(i),
            v => Err(TypeError{desc: format!("{:?} does not contain an int value", v), v: v}),
        }
    }

    pub fn uint(self) -> Result<u64, TypeError> {
        use Value::*;
        match self {
            Uint8(i) => Ok(i as u64),
            Uint16(i) => Ok(i as u64),
            Uint32(i) => Ok(i as u64),
            Uint64(i) => Ok(i),
            v => Err(TypeError{desc: format!("{:?} does not contain a uint value", v), v: v}),
        }
    }

    pub fn float(self) -> Result<f64, TypeError> {
        use Value::*;
        match self {
            Float32(f) => Ok(f as f64),
            Float64(f) => Ok(f),
            v => Err(TypeError{desc: format!("{:?} does not contain a float value", v), v: v}),
        }
    }

    pub fn string(self) -> Result<string::String, TypeError> {
        match self {
            Value::String(s) => Ok(s),
            v => Err(TypeError{desc: format!("{:?} does not contain a string value", v), v: v}),
        }
    }

    pub fn binary(self) -> Result<Vec<u8>, TypeError> {
        match self {
            Value::Binary(data) => Ok(data),
            v => Err(TypeError{desc: format!("{:?} does not contain a binary value", v), v: v}),
        }
    }

    pub fn array(self) -> Result<Vec<Value>, TypeError> {
        match self {
            Value::Array(ar) => Ok(ar),
            v => Err(TypeError{desc: format!("{:?} does not contain an array value", v), v: v}),
        }
    }

    pub fn map(self) -> Result<ValueMap, TypeError> {
        match self {
            Value::Map(m) => Ok(m),
            v => Err(TypeError{desc: format!("{:?} does not contain a map value", v), v: v}),
        }
    }

    pub fn extended_type(&self) -> Option<i8> {
        match *self {
            Value::Extended(x, _) => Some(x),
            _ => None,
        }
    }

    pub fn extended_data(self) -> Option<Vec<u8>> {
        match self {
            Value::Extended(_, data) => Some(data),
            _ => None,
        }
    }

    pub fn extended<T>(self) -> Result<T, TypeError> {
        use Value::*;
        match self {
            Extended(_, data) => Ok(unsafe { std::mem::transmute_copy(&data[0]) }),
            v => Err(TypeError{desc: format!("{:?} does not contain an extended value", v), v: v})
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct ValueMap(pub Vec<(Value, Value)>);

impl ValueMap {
    /// Retrieve a value from the map.
    pub fn get<T: IntoValue>(&self, key: T) -> Option<&Value> {
        let key = key.into_value();
        self.0.iter().find(|&&(ref k, _)| *k == key).map(|&(_, ref v)| v)
    }

    pub fn get_array<T: IntoValue>(&self, key: T) -> Option<Vec<Value>> {
        self.get(key).map(|v| v.clone().array().unwrap())
    }

    pub fn get_bool<T: IntoValue>(&self, key: T) -> Option<bool> {
        self.get(key).map(|v| v.clone().bool().unwrap())
    }

    pub fn get_string<T: IntoValue>(&self, key: T) -> Option<String> {
        self.get(key).map(|v| v.clone().string().unwrap())
    }

    /// Returns the number of key/value pairs in the map.
    pub fn len(&self) -> usize { self.0.len() }
}

/// A trait for types that can be written via MessagePack. This is mostly
/// a convenience to avoid having to wrap them yourself each time.
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
impl IntoValue for &'static str { fn into_value(self) -> Value { Value::String(String::from(self)) } }

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
        macro_rules! read_be_int(
            ($src:expr, $int:ident, $s:expr) => ({
                let mut val: $int = 0;
                for (i, next) in $src.by_ref().take($s).bytes().enumerate() {
                    val += (try!(next) as $int) << ($s - ((i + 1) as usize)) * 8;
                }
                val
            })
        );

        // Reads a big-endian float from the provided reader. Floats are trickier because they need
        // to be kept in raw-byte form and transmuted to the proper type, and there's no automatic
        // way to determine the size of the data type, so that needs to be passed in manually.
        macro_rules! read_be_float(
            ($src:expr, $f:ident, $s:expr) => ({
                let mut bytes: [u8; $s] = [0; $s];
                for (i, next) in $src.by_ref().take($s).bytes().enumerate() {
                    bytes[i] = try!(next);
                }
                if cfg!(target_endian = "little") {
                    bytes.reverse();
                }
                unsafe { std::mem::transmute::<[u8; $s], $f>(bytes) }
            });
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
            b @ byte::FIXINT_POS_RANGE_START...byte::FIXINT_POS_RANGE_END => Ok(Int8((b & 0b01111111) as i8)),

            b @ byte::FIXMAP_RANGE_START...byte::FIXMAP_RANGE_END => {
                let n = (b & 0b00001111) as usize;
                let mut m = Vec::with_capacity(n);
                for _ in 0..n {
                    m.push((try!(self.read_value()), try!(self.read_value())));
                }
                Ok(Map(ValueMap(m)))
            }

            b @ byte::FIXARRAY_RANGE_START...byte::FIXARRAY_RANGE_END => {
                let n = (b & 0b00001111) as usize;
                let mut ar = Vec::with_capacity(n);
                for _ in 0..n {
                    ar.push(try!(self.read_value()));
                }
                Ok(Array(ar))
            },

            b @ byte::FIXSTR_RANGE_START...byte::FIXSTR_RANGE_END => {
                let n = (b & 0b00011111) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::NIL => Ok(Nil),

            byte::FALSE => Ok(Boolean(false)),
            byte::TRUE => Ok(Boolean(true)),

            byte::U8 => Ok(Uint8(read_be_int!(self.reader, u8, 1))),
            byte::U16 => Ok(Uint16(read_be_int!(self.reader, u16, 2))),
            byte::U32 => Ok(Uint32(read_be_int!(self.reader, u32, 4))),
            byte::U64 => Ok(Uint64(read_be_int!(self.reader, u64, 8))),

            byte::I8 => Ok(Int8(read_be_int!(self.reader, i8, 1))),
            byte::I16 => Ok(Int16(read_be_int!(self.reader, i16, 2))),
            byte::I32 => Ok(Int32(read_be_int!(self.reader, i32, 4))),
            byte::I64 => Ok(Int64(read_be_int!(self.reader, i64, 8))),

            byte::F32 => Ok(Float32(read_be_float!(self.reader, f32, 4))),
            byte::F64 => Ok(Float64(read_be_float!(self.reader, f64, 8))),

            b if (b >> 5) == 0b00000101 => {
                let n = (b & 0b00011111) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::STR8 => {
                let n = read_be_int!(self.reader, u8, 1) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::STR16 => {
                let n = read_be_int!(self.reader, u16, 2) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::STR32 => {
                let n = read_be_int!(self.reader, u32, 4) as usize;
                match string::String::from_utf8(read_exact!(self.reader, n)) {
                    Ok(s) => Ok(String(s)),
                    Err(_) => panic!("received invalid utf-8"),
                }
            },

            byte::BIN8 => {
                let n = read_be_int!(self.reader, u8, 1) as usize;
                Ok(Binary(read_exact!(self.reader, n)))
            },

            byte::BIN16 => {
                let n = read_be_int!(self.reader, u16, 2) as usize;
                Ok(Binary(read_exact!(self.reader, n)))
            },

            byte::BIN32 => {
                let n = read_be_int!(self.reader, u32, 4) as usize;
                Ok(Binary(read_exact!(self.reader, n)))
            },

            b if (b >> 4) == 0b00001001 => {
                let n = (b & 0b00001111) as usize;
                let mut ar = Vec::with_capacity(n);
                for _ in 0..n {
                    ar.push(try!(self.read_value()));
                }
                Ok(Array(ar))
            },

            byte::AR16 => {
                let n = read_be_int!(self.reader, u16, 2) as usize;
                let mut ar = Vec::with_capacity(n);
                for _ in 0..n {
                    ar.push(try!(self.read_value()));
                }
                Ok(Array(ar))
            },

            byte::AR32 => {
                let n = read_be_int!(self.reader, u32, 4) as usize;
                let mut ar = Vec::with_capacity(n);
                for _ in 0..n {
                    ar.push(try!(self.read_value()));
                }
                Ok(Array(ar))
            },

            b if (b >> 4) == 0b00001000 => {
                let n = (b & 0b00001111) as usize;
                let mut m = Vec::with_capacity(n);
                for _ in 0..n {
                    m.push((try!(self.read_value()), try!(self.read_value())));
                }
                Ok(Map(ValueMap(m)))
            },

            byte::MAP16 => {
                let n = read_be_int!(self.reader, u16, 2) as usize;
                let mut m = Vec::with_capacity(n);
                for _ in 0..n {
                    m.push((try!(self.read_value()), try!(self.read_value())));
                }
                Ok(Map(ValueMap(m)))
            },

            byte::MAP32 => {
                let n = read_be_int!(self.reader, u32, 4) as usize;
                let mut m = Vec::with_capacity(n);
                for _ in 0..n {
                    m.push((try!(self.read_value()), try!(self.read_value())));
                }
                Ok(Map(ValueMap(m)))
            },

            // Extension types.
            byte::FIXEXT1 => Ok(Extended(read_be_int!(self.reader, i8, 1), read_exact!(self.reader, 1))),
            byte::FIXEXT2 => Ok(Extended(read_be_int!(self.reader, i8, 1), read_exact!(self.reader, 2))),
            byte::FIXEXT4 => Ok(Extended(read_be_int!(self.reader, i8, 1), read_exact!(self.reader, 4))),
            byte::FIXEXT8 => Ok(Extended(read_be_int!(self.reader, i8, 1), read_exact!(self.reader, 8))),
            byte::FIXEXT16 => Ok(Extended(read_be_int!(self.reader, i8, 1), read_exact!(self.reader, 16))),

            byte::EXT8 => {
                let n = read_be_int!(self.reader, u8, 1) as usize;
                Ok(Extended(read_be_int!(self.reader, i8, 1), read_exact!(self.reader, n)))
            },

            byte::EXT16 => {
                let n = read_be_int!(self.reader, u16, 2) as usize;
                Ok(Extended(read_be_int!(self.reader, i8, 1), read_exact!(self.reader, n)))
            },

            byte::EXT32 => {
                let n = read_be_int!(self.reader, u32, 4) as usize;
                Ok(Extended(read_be_int!(self.reader, i8, 1), read_exact!(self.reader, n)))
            },

            b @ byte::FIXINT_NEG_RANGE_START...byte::FIXINT_NEG_RANGE_END => Ok(Int8((b & 0b00011111) as i8)),

            b => {
                self.next_byte = Some(b);
                Err(ReadError::Unrecognized(b))
            },
        }
    }
}

/// Convenience wrapper for `write_value()`.
pub fn write<W: Write, V: IntoValue>(dest: &mut W, val: V) -> Result<(), WriteError> {
    write_value(dest, val.into_value())
}

/// Write any value as an Extended type. On success, returns the number of bytes written.
///
/// The `val` parameter will be automatically converted to its byte representation.
pub fn write_ext<W: Write, T>(dest: &mut W, id: i8, val: T) -> Result<usize, WriteError> {
    let data: &[u8] = unsafe {
        slice::from_raw_parts(&val as *const _ as *const u8, std::mem::size_of::<T>())
    };
    try!(write_value(dest, Value::Extended(id, data.to_vec())));
    Ok(data.len())
}

/// Write a message in MessagePack format for the given value.
pub fn write_value<W: Write>(dest: &mut W, val: Value) -> Result<(), WriteError> {
    use Value::*;
    use std::mem::transmute;

    // Convert an integer to its big-endian byte representation.
    macro_rules! be_int(
        ($x:expr, $n:ident, $s:expr) => ({
            unsafe { transmute::<_, [u8; $s]>(($x as $n).to_be()) }
        })
    );

    // Convert a float to its big-endian byte representation.
    macro_rules! be_float(
        ($x:expr, $f:ident, $s:expr) => ({
            let mut bytes = unsafe { transmute::<$f, [u8; $s]>($x) };
            if cfg!(target_endian = "little") {
                bytes.reverse();
            }
            bytes
        })
    );

    // Create a slice out of a leading byte with any number of slices appended to it.
    macro_rules! data(
        ($b:expr; $($data:expr),+) => ({
            let mut v = Vec::with_capacity({ let mut size = 1; $( size += $data.len(); )+ size });
            v.push($b); $( v.extend_from_slice(&$data); )+ &v.into_boxed_slice()
        })
    );

    match val {
        Nil => try!(dest.write_all(&[byte::NIL])),

        Boolean(false) => try!(dest.write_all(&[byte::FALSE])),
        Boolean(true) => try!(dest.write_all(&[byte::TRUE])),

        Uint8(x) => try!(dest.write_all(&[byte::U8, x])),
        Uint16(x) => try!(dest.write_all(data![byte::U16; be_int!(x, u16, 2)])),
        Uint32(x) => try!(dest.write_all(data![byte::U32; be_int!(x, u32, 4)])),
        Uint64(x) => try!(dest.write_all(data![byte::U64; be_int!(x, u64, 8)])),

        Int8(x) => try!(dest.write_all(&[byte::I8, x as u8])),
        Int16(x) => try!(dest.write_all(data![byte::I16; be_int!(x, i16, 2)])),
        Int32(x) => try!(dest.write_all(data![byte::I32; be_int!(x, i32, 4)])),
        Int64(x) => try!(dest.write_all(data![byte::I64; be_int!(x, i64, 8)])),

        Float32(x) => try!(dest.write_all(data![byte::F32; be_float!(x, f32, 4)])),
        Float64(x) => try!(dest.write_all(data![byte::F64; be_float!(x, f64, 8)])),

        String(s) => {
            let bytes = s.as_bytes();
            let n = bytes.len();
            try!(match n {
                0...31 => Ok(try!(dest.write_all(data![(0b10100000 | n) as u8; bytes]))), // fixstr
                32...255 => Ok(try!(dest.write_all(data![byte::STR8; [n as u8], bytes]))), // str 8
                256...65535 => Ok(try!(dest.write_all(data![byte::STR16; be_int!(n, u16, 2), bytes]))), // str 16
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::STR32; be_int!(n, u32, 4), bytes]))), // str 32
                _ => Err(WriteError::TooMuchData(n)),
            });
        },

        Binary(b) => {
            let n = b.len();
            try!(match n {
                0...255 => Ok(try!(dest.write_all(data![byte::BIN8; be_int!(n, u8, 1), b.as_slice()]))), // bin 8
                256...65535 => Ok(try!(dest.write_all(data![byte::BIN16; be_int!(n, u16, 2), b.as_slice()]))), // bin 16
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::BIN32; be_int!(n, u32, 4), b.as_slice()]))), // bin 32
                _ => Err(WriteError::TooMuchData(n)),
            });
        },

        Array(values) => {
            let n = values.len();
            try!(match n {
                0...15 => Ok(try!(dest.write_all(&[(0b10010000 | n) as u8]))), // fixarray
                16...65535 => Ok(try!(dest.write_all(data![byte::AR16; be_int!(n, u16, 2)]))), // 16 array
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::AR32; be_int!(n, u32, 4)]))), // 32 array
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
                16...65535 => Ok(try!(dest.write_all(data![byte::MAP16; be_int!(n, u16, 2)]))), // 16 map
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::MAP32; be_int!(n, u32, 4)]))), // 32 map
                _ => Err(WriteError::TooMuchData(n)),
            });
            for (k, v) in entries.0.into_iter() {
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
                256...65535 => Ok(try!(dest.write_all(data![byte::MAP16; be_int!(n, u16, 2)]))), // ext 16
                65536...4294967295 => Ok(try!(dest.write_all(data![byte::MAP32; be_int!(n, u32, 4)]))), // ext 32
                _ => Err(WriteError::TooMuchData(n)),
            });
            try!(dest.write_all(&[id as u8]));
            try!(dest.write_all(data.as_slice()));
        },
    }
    Ok(())
}

/* -- Errors -- */

#[derive(Debug)]
pub struct TypeError {
    v: Value,
    desc: String,
}

impl TypeError {
    /// Retrieve the value that caused the type error.
    pub fn value(self) -> Value { self.v }
}

impl Error for TypeError {
    fn description(&self) -> &str { self.desc.as_str() }
    fn cause(&self) -> Option<&Error> { None }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(fmt, "{}", self)
    }
}

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

impl From<io::Error> for WriteError {
    fn from(err: io::Error) -> WriteError {
        WriteError::Io(err)
    }
}

// For some reason the Error implementation barks if you don't do this.
impl std::fmt::Display for WriteError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(fmt, "{}", self)
    }
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

impl From<io::Error> for ReadError {
    fn from(err: io::Error) -> ReadError {
        ReadError::Io(err)
    }
}

// For some reason the Error implementation barks if you don't do this.
impl std::fmt::Display for ReadError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(fmt, "{}", self)
    }
}

#[cfg(test)]
mod test {
    extern crate rand;

    use std::io::{self, Read, Write};
    use std::mem;
    use std::slice;
    use std::string;
    use std::sync::mpsc::{channel, Sender, Receiver};
    use self::rand::{Rng, StdRng};
    use super::{IntoValue, write_value};

    pub struct ChanReader(pub Receiver<u8>);

    impl Read for ChanReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            for i in 0..buf.len() {
                match self.0.recv() {
                    Ok(byte) => buf[i] = byte,
                    Err(..) => return Ok(i),
                }
            }
            Ok(buf.len())
        }
    }

    pub struct ChanWriter(pub Sender<u8>);

    impl Write for ChanWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() == 0 {
                Ok(0)
            } else {
                match self.0.send(buf[0]) {
                    Ok(()) => Ok(1),
                    Err(err) => Err(io::Error::new(io::ErrorKind::BrokenPipe, err)),
                }
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
            for byte in buf.iter() {
                if let Err(err) = self.0.send(*byte) {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, err));
                }
            }
            Ok(())
        }
    }

    const LETTERS: &'static [char] = &['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z'];

    fn test<T: IntoValue>(arg: T) {
        let val = arg.into_value();
        let (tx, rx) = channel();
        write_value(&mut ChanWriter(tx), val.clone()).unwrap();
        assert_eq!(super::Reader::new(&mut ChanReader(rx)).read_value().unwrap(), val);
    }

    fn random_string(n: usize) -> string::String {
        let mut rng = StdRng::new().unwrap();

        let mut s = string::String::with_capacity(n);
        for _ in 0..n {
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

    #[repr(packed)]
    struct CustomStruct {
        a: i8,
        b: i16,
    }

    /// Note that extended values cannot include pointers to outside data.
    #[test] fn test_extended() {
        let (tx, rx) = channel();
        let written = super::write_ext(&mut ChanWriter(tx), 13, CustomStruct{a: 13, b: 42}).unwrap();
        assert_eq!(written, 3);

        let value = super::Reader::new(&mut ChanReader(rx)).read_value().unwrap();
        assert_eq!(value.extended_type().unwrap(), 13);

        let x: CustomStruct = value.extended().unwrap();
        assert_eq!(x.a, 13);
        assert_eq!(x.b, 42);
    }
}
