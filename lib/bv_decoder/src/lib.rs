use base64::Engine as _;
use bit_vec::BitVec;
use std::fmt::{ self };

pub fn b64url_to_bitvec(input: &str) -> Result<BitVec, base64::DecodeError> {
  // Strip any existing padding and decode using URL_SAFE_NO_PAD
  let stripped = input.trim_end_matches('=');
  let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(stripped)?;

  // Convert bytes to BitVec (MSB first)
  let mut bitvec = BitVec::new();
  for byte in decoded {
    for i in (0..8).rev() {
      bitvec.push(((byte >> i) & 1) == 1);
    }
  }

  Ok(bitvec)
}

#[derive(Clone, Debug)]
pub struct BvWeights {
  pub bv: BitVec,
  pub weights: Vec<u64>,
}

impl fmt::Display for BvWeights {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "Binary: {} Voted: {} Eligible: {}", self.bv, self.voted_weight(), self.eligible_weight())
  }
}

impl BvWeights {
  pub fn from_bitvec(bv: BitVec, weights: &Vec<u64>) -> BvWeights {
    BvWeights { bv, weights: weights.clone() }
  }

  pub fn from_b64url(b64url: &str, weights: &Vec<u64>) -> Result<BvWeights, base64::DecodeError> {
    Ok(BvWeights { bv: b64url_to_bitvec(b64url)?, weights: weights.clone() })
  }

  pub fn eligible_weight(&self) -> u64 {
    self.weights.iter().sum()
  }

  pub fn voted_weight(&self) -> u64 {
    let mut voted: u64 = 0;
    for i in 0..self.weights.len() {
      match self.bv.len().checked_sub(i + 1) {
        Some(pos) => {
          if self.bv.get(pos).unwrap_or(false) {
            voted += self.weights.get(i).unwrap_or(&0).clone();
          }
        }
        None => (),
      }
    }
    return voted;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_block6() {
    // Note: This example shows partial byte handling (not standard base64)
    let input = "HfP-"; // Decodes to 0x1d 0xf3 0xfe (24 bits)

    let result = b64url_to_bitvec(input).unwrap();

    // Manually create expected bits
    let mut expected = BitVec::new();
    // this block has 21 witnesses eligible, but the function pad it with 3 extra zeros so there's 24 bits here
    // bv: 11101111 10011111 11110
    let bits = vec![
      false,
      false,
      false,
      true,
      true,
      true,
      false,
      true,
      true,
      true,
      true,
      true,
      false,
      false,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      false
    ];
    expected.extend(bits.iter().copied());

    assert_eq!(result, expected);
    assert_eq!(result.len(), 24);
    assert_eq!(BvWeights::from_bitvec(result, &vec![10; 21]).voted_weight(), 170);
  }

  #[test]
  fn test_block138() {
    let input = "B__7";
    let result = b64url_to_bitvec(input).unwrap();
    let mut expected = BitVec::new();
    // 20 witnesses total, the last witness "vaultec.vsc" did not attest
    // bv: 01111111 11111111 1011
    let bits = vec![
      false,
      false,
      false,
      false,
      false,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      false,
      true,
      true
    ];
    expected.extend(bits.iter().copied());

    assert_eq!(result, expected);
    assert_eq!(result.len(), 24);

    // weights for epoch 23
    let weights = vec![
      2000000,
      2000000,
      2000000,
      2000000,
      2000100,
      2000000,
      2950000,
      2007000,
      2025000,
      2000000,
      2000100,
      2100000,
      2000000,
      2000000,
      2002000,
      2106080,
      2000000,
      2049738,
      2000000,
      2000000
    ];
    assert_eq!(BvWeights::from_bitvec(result, &weights).voted_weight(), 37240018);
  }

  #[test]
  fn test_block138_25_members() {
    let input = "B__7";
    let result = b64url_to_bitvec(input).unwrap();
    let mut expected = BitVec::new();
    // 25 witnesses total, the last 6 witnesses did not attest
    // bv: 00000011 11111111 11111101 1
    let bits = vec![
      false,
      false,
      false,
      false,
      false,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      true,
      false,
      true,
      true
    ];
    expected.extend(bits.iter().copied());

    assert_eq!(result, expected);
    assert_eq!(result.len(), 24);

    // weights for epoch 23 extended to 25 members
    let weights = vec![
      2000000,
      2000000,
      2000000,
      2000000,
      2000100,
      2000000,
      2950000,
      2007000,
      2025000,
      2000000,
      2000100,
      2100000,
      2000000,
      2000000,
      2002000,
      2106080,
      2000000,
      2049738,
      2000000,
      2000000,
      2000000,
      2000000,
      2000000,
      2000000,
      2000000
    ];
    assert_eq!(BvWeights::from_bitvec(result, &weights).voted_weight(), 37240018);
  }

  #[test]
  fn test_testnet_epoch1000() {
    // testnet epoch 1000: df2d9e60cca350c79d8a50eb2d61306a6e029258
    let bv_str = "-9s";
    let bv_weights = BvWeights::from_b64url(bv_str, &vec![13, 13, 13, 6, 13, 13, 13, 10, 9, 9, 13, 10, 13, 9, 9, 9]).unwrap();
    assert_eq!(bv_weights.eligible_weight(), 175);
    assert_eq!(bv_weights.voted_weight(), 136);

    // Manually create expected bits
    let mut expected = BitVec::new();
    let bits = vec![true, true, true, true, true, false, true, true, true, true, false, true, true, false, true, true];
    expected.extend(bits.iter().copied());
    assert_eq!(bv_weights.bv, expected);
  }

  #[test]
  fn test_empty() {
    let bv_weights = BvWeights::from_b64url("", &vec![]).unwrap();
    assert_eq!(bv_weights.eligible_weight(), 0);
    assert_eq!(bv_weights.voted_weight(), 0);
  }
}
