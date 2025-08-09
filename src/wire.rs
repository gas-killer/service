use bytes::{Buf, BufMut};
use commonware_codec::{EncodeSize, Error, Read, ReadExt, Write};

const SIGNATURE_BYTES: usize = 32;

/// Represents a top-level message for the Aggregation protocol,
/// typically sent over a dedicated aggregation communication channel.
///
/// It encapsulates a specific round number, target contract information, and a payload containing
/// the actual aggregation protocol message content.
#[derive(Clone, Debug, PartialEq)]
pub struct Aggregation {
    pub round: u64,
    pub target_contract: String,  // Target contract address (hex string)
    pub target_function: String,  // Target function selector (hex string)
    pub function_params: Vec<u8>, // Encoded function parameters
    pub payload: Option<aggregation::Payload>,
}

impl Write for Aggregation {
    fn write(&self, buf: &mut impl BufMut) {
        self.round.write(buf);
        (self.target_contract.len() as u32).write(buf);
        buf.put_slice(self.target_contract.as_bytes());
        (self.target_function.len() as u32).write(buf);
        buf.put_slice(self.target_function.as_bytes());
        (self.function_params.len() as u32).write(buf);
        buf.put_slice(&self.function_params);
        self.payload.write(buf);
    }
}

impl Read for Aggregation {
    type Cfg = ();

    fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, Error> {
        let round = u64::read(buf)?;

        let target_contract_len = u32::read(buf)? as usize;
        if buf.remaining() < target_contract_len {
            return Err(Error::EndOfBuffer);
        }
        let mut target_contract_bytes = vec![0u8; target_contract_len];
        buf.copy_to_slice(&mut target_contract_bytes);
        let target_contract = String::from_utf8(target_contract_bytes)
            .map_err(|_| Error::Invalid("target_contract", "decoding from utf8 failed"))?;

        let target_function_len = u32::read(buf)? as usize;
        if buf.remaining() < target_function_len {
            return Err(Error::EndOfBuffer);
        }
        let mut target_function_bytes = vec![0u8; target_function_len];
        buf.copy_to_slice(&mut target_function_bytes);
        let target_function = String::from_utf8(target_function_bytes)
            .map_err(|_| Error::Invalid("target_function", "decoding from utf8 failed"))?;

        let function_params_len = u32::read(buf)? as usize;
        if buf.remaining() < function_params_len {
            return Err(Error::EndOfBuffer);
        }
        let mut function_params = vec![0u8; function_params_len];
        buf.copy_to_slice(&mut function_params);

        let payload = Option::<aggregation::Payload>::read(buf)?;
        Ok(Self {
            round,
            target_contract,
            target_function,
            function_params,
            payload,
        })
    }
}

impl EncodeSize for Aggregation {
    fn encode_size(&self) -> usize {
        self.round.encode_size()
            + 4
            + self.target_contract.len()
            + 4
            + self.target_function.len()
            + 4
            + self.function_params.len()
            + self.payload.encode_size()
    }
}

pub mod aggregation {

    use bytes::{Buf, BufMut};
    use commonware_codec::{EncodeSize, Error, Read, ReadExt, ReadRangeExt, Write};

    use super::SIGNATURE_BYTES;

    /// Defines the different types of messages exchanged during the aggregation protocol.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Payload {
        /// Message sent by orchestrator to start aggregation
        Start,
        /// Sent by signer to all others
        Signature(Vec<u8>),
    }

    impl Write for Payload {
        fn write(&self, buf: &mut impl BufMut) {
            match self {
                Payload::Start => {
                    buf.put_u8(0);
                }
                Payload::Signature(signature) => {
                    buf.put_u8(1);
                    signature.write(buf);
                }
            }
        }
    }

    impl Read for Payload {
        type Cfg = ();

        fn read_cfg(buf: &mut impl Buf, _: &()) -> Result<Self, Error> {
            let tag = u8::read(buf)?;
            let result = match tag {
                0 => Payload::Start,
                1 => Payload::Signature(Vec::<u8>::read_range(buf, 1..(SIGNATURE_BYTES + 1))?),
                _ => return Err(Error::InvalidEnum(tag)),
            };
            Ok(result)
        }
    }

    impl EncodeSize for Payload {
        fn encode_size(&self) -> usize {
            1 + match self {
                Payload::Start => 0,
                Payload::Signature(signature) => signature.encode_size(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::hex;

    const SAMPLE_SIGNATURE_HEX: &str =
        "4ffa4441848335dace97935d3c167d212fe5563c1ce9a626cc6d69b4fe06449c";

    #[test]
    fn test_aggregation_start_codec() {
        let original = Aggregation {
            round: 1,
            target_contract: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1".to_string(),
            target_function: "0xa9059cbb".to_string(), // transfer function selector
            function_params: vec![0x01, 0x02, 0x03, 0x04], // sample encoded params
            payload: Some(aggregation::Payload::Start),
        };
        let mut buf = Vec::with_capacity(original.encode_size());
        original.write(&mut buf);
        let decoded = Aggregation::read(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_aggregation_signature_codec() {
        let original = Aggregation {
            round: 1,
            target_contract: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1".to_string(),
            target_function: "0xa9059cbb".to_string(), // transfer function selector
            function_params: vec![0x01, 0x02, 0x03, 0x04], // sample encoded params
            payload: Some(aggregation::Payload::Signature(
                hex::decode(SAMPLE_SIGNATURE_HEX).expect("hex decode failed"),
            )),
        };
        let mut buf = Vec::with_capacity(original.encode_size());
        original.write(&mut buf);
        let decoded = Aggregation::read(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(original, decoded);
    }
}
