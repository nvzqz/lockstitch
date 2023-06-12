#[cfg(target_arch = "x86")]
pub use core::arch::x86::*;

#[cfg(target_arch = "x86_64")]
pub use core::arch::x86_64::*;

pub use self::__m128i as AesBlock;

macro_rules! zero {
    () => {{
        unsafe { _mm_setzero_si128() }
    }};
}

pub(crate) use zero;

macro_rules! load {
    ($bytes:expr) => {{
        let block: &[u8] = $bytes; // N.B.: loads are broken without this aliasing
        unsafe { _mm_loadu_si128(block.as_ptr() as *const __m128i) }
    }};
}

pub(crate) use load;

macro_rules! load_64x2 {
    ($a:expr, $b:expr) => {{
        unsafe { _mm_set_epi64x($b.try_into().unwrap(), $a.try_into().unwrap()) }
    }};
}

pub(crate) use load_64x2;

macro_rules! store {
    ($bytes:expr, $block:expr) => {{
        unsafe { _mm_storeu_si128($bytes.as_mut_ptr() as *mut __m128i, $block) };
    }};
}

pub(crate) use store;

macro_rules! xor {
    ($a:expr, $b:expr) => {{
        unsafe { _mm_xor_si128($a, $b) }
    }};
    ($a:expr, $b:expr, $c:expr) => {{
        let b = xor!($b, $c);
        unsafe { _mm_xor_si128($a, b) }
    }};
}

pub(crate) use xor;

macro_rules! and {
    ($a:expr, $b:expr) => {{
        unsafe { _mm_and_si128($a, $b) }
    }};
}

pub(crate) use and;

macro_rules! enc {
    ($a:expr, $b:expr) => {{
        unsafe { _mm_aesenc_si128($a, $b) }
    }};
}

pub(crate) use enc;
