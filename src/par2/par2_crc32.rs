const CRC32_POLY: u32 = 0xEDB88320;

// Multiply two polynomials modulo the CRC32 polynomial
fn multiply_mod(mut a: u32, mut b: u32) -> u32 {
    let mut result = 0u32;
    for _ in 0..32 {
        if b & 1 == 1 {
            result ^= a;
        }
        let carry = a & 0x80000000;
        a <<= 1;
        if carry != 0 {
            a ^= CRC32_POLY;
        }
        b >>= 1;
    }
    result
}

// Compute x^(8n) mod CRC32 polynomial
pub fn xpow8n(n: u64) -> u32 {
    let mut result = 1u32;
    let mut base = 0x80000000u32;
    let mut exp = n * 8;

    while exp > 0 {
        if exp & 1 == 1 {
            result = multiply_mod(result, base);
        }
        base = multiply_mod(base, base);
        exp >>= 1;
    }
    result
}

pub fn multiply(a: u32, b: u32) -> u32 {
    multiply_mod(a, b)
}

pub fn combine(crc1: u32, crc2: u32, len2: u64) -> u32 {
    if len2 == 0 {
        return crc1;
    }
    multiply(crc1, xpow8n(len2)) ^ crc2
}

pub fn zero_unpad(crc: u32, unpad_len: u64) -> u32 {
    multiply(crc, xpow8n(unpad_len))
}
