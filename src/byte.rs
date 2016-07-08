pub const FIXINT_POS_RANGE_START: u8 = 0x00;
pub const FIXINT_POS_RANGE_END: u8   = 0x7F;

pub const FIXMAP_RANGE_START: u8 = 0x80;
pub const FIXMAP_RANGE_END: u8   = 0x8F;

pub const FIXARRAY_RANGE_START: u8 = 0x90;
pub const FIXARRAY_RANGE_END: u8   = 0x9F;

pub const FIXSTR_RANGE_START: u8 = 0xA0;
pub const FIXSTR_RANGE_END: u8   = 0xBF;

pub const NIL:   u8 = 0xC0;

pub const FALSE: u8 = 0xC2;
pub const TRUE:  u8 = 0xC3;

pub const U8:    u8 = 0xCC;
pub const U16:   u8 = 0xCD;
pub const U32:   u8 = 0xCE;
pub const U64:   u8 = 0xCF;

pub const I8:    u8 = 0xD0;
pub const I16:   u8 = 0xD1;
pub const I32:   u8 = 0xD2;
pub const I64:   u8 = 0xD3;
pub const F32:   u8 = 0xCA;
pub const F64:   u8 = 0xCB;

pub const STR8:  u8 = 0xD9;
pub const STR16: u8 = 0xDA;
pub const STR32: u8 = 0xDB;

pub const BIN8:  u8 = 0xC4;
pub const BIN16: u8 = 0xC5;
pub const BIN32: u8 = 0xC6;

pub const AR16:  u8 = 0xDC;
pub const AR32:  u8 = 0xDD;

pub const MAP16: u8 = 0xDE;
pub const MAP32: u8 = 0xDF;

pub const FIXEXT1:  u8 = 0xD4;
pub const FIXEXT2:  u8 = 0xD5;
pub const FIXEXT4:  u8 = 0xD6;
pub const FIXEXT8:  u8 = 0xD7;
pub const FIXEXT16: u8 = 0xD8;

pub const EXT8:  u8 = 0xC7;
pub const EXT16: u8 = 0xC8;
pub const EXT32: u8 = 0xC9;

pub const FIXINT_NEG_RANGE_START: u8 = 0xE0;
pub const FIXINT_NEG_RANGE_END: u8   = 0xFF;

pub fn desc(b: u8) -> &'static str {
    match b {
        FIXINT_POS_RANGE_START...FIXINT_POS_RANGE_END => "POS FIXINT",
        FIXMAP_RANGE_START...FIXMAP_RANGE_END => "FIXMAP",
        FIXARRAY_RANGE_START...FIXARRAY_RANGE_END => "FIXARRAY",
        FIXSTR_RANGE_START...FIXSTR_RANGE_END => "FIXSTR",
        NIL => "NIL",
        FALSE => "FALSE",
        TRUE => "TRUE",
        U8 => "U8",
        U16 => "U16",
        U32 => "U32",
        U64 => "U64",
        I8 => "I8",
        I16 => "I16",
        I32 => "I32",
        I64 => "I64",
        F32 => "F32",
        F64 => "F64",
        STR8 => "STR8",
        STR16 => "STR16",
        STR32 => "STR32",
        BIN8 => "BIN8",
        BIN16 => "BIN16",
        BIN32 => "BIN32",
        AR16 => "AR16",
        AR32 => "AR32",
        MAP16 => "MAP16",
        MAP32 => "MAP32",
        FIXEXT1 => "FIXEXT1",
        FIXEXT2 => "FIXEXT2",
        FIXEXT4 => "FIXEXT4",
        FIXEXT8 => "FIXEXT8",
        FIXEXT16 => "FIXEXT16",
        EXT8 => "EXT8",
        EXT16 => "EXT16",
        EXT32 => "EXT32",
        FIXINT_NEG_RANGE_START...FIXINT_NEG_RANGE_END => "NEG FIXINT",
        _ => "<unknown>",
    }
}
