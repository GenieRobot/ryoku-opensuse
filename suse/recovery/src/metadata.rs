//! Bounded raw metadata payloads stored only in the protected generation store.
//!
//! Manifests contain digests, never raw labels or ACLs. The ACL encoding is the
//! exact preimage of the existing `acl_sha256` field, including presence bits
//! so absence and an empty value remain distinct. No arbitrary xattr names are
//! accepted. Decoding performs no filesystem access or metadata application.

use crate::{EntryKind, EntryMetadata};

pub const MAX_METADATA_VALUE_BYTES: usize = 64 * 1024;
const DOMAIN: &[u8] = b"ryoku-posix-acl-v1\0";
const ACCESS: &[u8] = b"system.posix_acl_access";
const DEFAULT: &[u8] = b"system.posix_acl_default";
pub const MAX_ACL_PAYLOAD_BYTES: usize =
    DOMAIN.len() + ACCESS.len() + DEFAULT.len() + 2 * (1 + 1 + 8 + MAX_METADATA_VALUE_BYTES);

/// Raw Linux POSIX ACL xattrs. Values are opaque, bounded bytes; the target
/// filesystem remains responsible for validating ACL semantics when applied.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AclPayload {
    pub access: Option<Vec<u8>>,
    pub default: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidMetadataPayload;

impl std::fmt::Display for InvalidMetadataPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("metadata payload is malformed or exceeds its fixed bound")
    }
}
impl std::error::Error for InvalidMetadataPayload {}

impl AclPayload {
    /// `None` represents two absent ACLs and therefore no ACL blob/digest.
    pub fn encode(&self) -> Result<Option<Vec<u8>>, InvalidMetadataPayload> {
        if self.access.is_none() && self.default.is_none() {
            return Ok(None);
        }
        let mut bytes = DOMAIN.to_vec();
        for (name, value) in [(ACCESS, &self.access), (DEFAULT, &self.default)] {
            bytes.extend_from_slice(name);
            bytes.push(0);
            match value {
                None => {
                    bytes.push(0);
                    bytes.extend_from_slice(&0_u64.to_be_bytes());
                }
                Some(value) => {
                    if value.len() > MAX_METADATA_VALUE_BYTES {
                        return Err(InvalidMetadataPayload);
                    }
                    bytes.push(1);
                    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
                    bytes.extend_from_slice(value);
                }
            }
        }
        Ok(Some(bytes))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, InvalidMetadataPayload> {
        if bytes.len() > MAX_ACL_PAYLOAD_BYTES {
            return Err(InvalidMetadataPayload);
        }
        let mut remaining = bytes.strip_prefix(DOMAIN).ok_or(InvalidMetadataPayload)?;
        let access = decode_field(&mut remaining, ACCESS)?;
        let default = decode_field(&mut remaining, DEFAULT)?;
        if !remaining.is_empty() || (access.is_none() && default.is_none()) {
            return Err(InvalidMetadataPayload);
        }
        Ok(Self { access, default })
    }
}

fn decode_field(
    remaining: &mut &[u8],
    name: &[u8],
) -> Result<Option<Vec<u8>>, InvalidMetadataPayload> {
    *remaining = remaining.strip_prefix(name).ok_or(InvalidMetadataPayload)?;
    *remaining = remaining.strip_prefix(&[0]).ok_or(InvalidMetadataPayload)?;
    let (&present, rest) = remaining.split_first().ok_or(InvalidMetadataPayload)?;
    let length_bytes: [u8; 8] = rest
        .get(..8)
        .ok_or(InvalidMetadataPayload)?
        .try_into()
        .map_err(|_| InvalidMetadataPayload)?;
    let length =
        usize::try_from(u64::from_be_bytes(length_bytes)).map_err(|_| InvalidMetadataPayload)?;
    if length > MAX_METADATA_VALUE_BYTES || present > 1 || (present == 0 && length != 0) {
        return Err(InvalidMetadataPayload);
    }
    let rest = &rest[8..];
    let value = rest.get(..length).ok_or(InvalidMetadataPayload)?;
    *remaining = &rest[length..];
    Ok((present == 1).then(|| value.to_vec()))
}

pub(crate) fn entry_metadata(kind: &EntryKind) -> Option<&EntryMetadata> {
    match kind {
        EntryKind::Absent => None,
        EntryKind::Directory { metadata }
        | EntryKind::RegularFile { metadata, .. }
        | EntryKind::Symlink { metadata, .. } => Some(metadata),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_and_both_acl_values_round_trip_without_public_manifest_data() {
        for payload in [
            AclPayload { access: Some(vec![]), default: None },
            AclPayload { access: None, default: Some(vec![]) },
            AclPayload { access: Some(vec![0, 1, 255]), default: Some(vec![2, 0, 3]) },
            AclPayload { access: Some(vec![5; MAX_METADATA_VALUE_BYTES]), default: None },
        ] {
            let bytes = payload.encode().unwrap().unwrap();
            assert_eq!(AclPayload::decode(&bytes).unwrap(), payload);
        }
        assert_eq!(AclPayload::default().encode().unwrap(), None);
    }

    #[test]
    fn rejects_truncation_unknown_names_presence_flags_and_trailing_data() {
        let bytes = AclPayload { access: Some(vec![42]), default: None }.encode().unwrap().unwrap();
        for end in 0..bytes.len() {
            assert!(AclPayload::decode(&bytes[..end]).is_err());
        }
        let mut wrong = bytes.clone();
        wrong[DOMAIN.len()] = b'x';
        assert!(AclPayload::decode(&wrong).is_err());
        let mut wrong = bytes.clone();
        wrong[DOMAIN.len() + ACCESS.len() + 1] = 2;
        assert!(AclPayload::decode(&wrong).is_err());
        let mut wrong = bytes;
        wrong.push(0);
        assert!(AclPayload::decode(&wrong).is_err());
        assert!(
            AclPayload { access: Some(vec![0; MAX_METADATA_VALUE_BYTES + 1]), default: None }
                .encode()
                .is_err()
        );
    }

    #[test]
    fn encoding_preserves_the_existing_v1_acl_digest_preimage() {
        let payload = AclPayload { access: Some(vec![42]), default: None };
        assert_eq!(
            payload.encode().unwrap().unwrap(),
            b"ryoku-posix-acl-v1\0system.posix_acl_access\0\x01\0\0\0\0\0\0\0\x01\x2asystem.posix_acl_default\0\0\0\0\0\0\0\0\0\0"
        );
    }
}
