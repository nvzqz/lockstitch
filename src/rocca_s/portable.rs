pub use aes::Block as AesBlock;

macro_rules! zero {
    () => {{
        [0u8; 16].into()
    }};
}

pub(crate) use zero;

macro_rules! from_bytes {
    ($bytes:expr, $idx:expr) => {{
        let bytes: &Aligned<A16, _> = $bytes;
        *AesBlock::from_slice(&bytes[$idx])
    }};
}

pub(crate) use from_bytes;

macro_rules! to_bytes {
    ($bytes:expr, $idx:expr, $block:expr) => {{
        let bytes: &mut Aligned<A16, _> = $bytes;
        bytes[$idx].copy_from_slice(&$block);
    }};
}

pub(crate) use to_bytes;

macro_rules! xor {
    ($a:expr) => {$a};
    ($a:expr, $($rest:expr),*) => {
        xor_block($a, xor!($($rest), *))
    };
}

pub(crate) use xor;

pub fn xor_block(a: AesBlock, b: AesBlock) -> AesBlock {
    let mut out = AesBlock::default();
    for ((z, x), y) in out.iter_mut().zip(a).zip(b) {
        *z = x ^ y;
    }
    out
}

macro_rules! round {
    ($a:expr, $b:expr) => {{
        let mut out = $a;
        aes::hazmat::cipher_round(&mut out, &$b);
        out
    }};
}

pub(crate) use round;