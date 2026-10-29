//! Bounded raster attachments. Bytes and their declared type are validated
//! before they enter private conversation state or an adapter prompt.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_IMAGES: usize = 4;
pub(crate) const MAX_TOTAL_BYTES: usize = 3 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub data: String,
}
impl Image {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(Error::Limit);
        }
        let mime_type = kind(bytes).ok_or(Error::InvalidRequest)?;
        Ok(Self { mime_type: mime_type.into(), data: crate::http::base64(bytes) })
    }
    pub(crate) fn validate(&self) -> Result<usize> {
        if self.data.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err(Error::Limit);
        }
        let bytes = decode(&self.data)?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(Error::Limit);
        }
        if kind(&bytes) != Some(self.mime_type.as_str()) {
            return Err(Error::InvalidRequest);
        }
        Ok(bytes.len())
    }
}
pub(crate) fn validate(images: &[Image]) -> Result<()> {
    if images.len() > MAX_IMAGES {
        return Err(Error::Limit);
    }
    validate_history(images)
}
pub(crate) fn validate_history(images: &[Image]) -> Result<()> {
    if images.len() > 64 {
        return Err(Error::Limit);
    }
    let mut total = 0usize;
    for image in images {
        total = total.checked_add(image.validate()?).ok_or(Error::Limit)?;
    }
    if total > MAX_TOTAL_BYTES {
        return Err(Error::Limit);
    }
    Ok(())
}
fn kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
pub(crate) fn decode(data: &str) -> Result<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    if data.is_empty() || !data.len().is_multiple_of(4) {
        return Err(Error::InvalidRequest);
    }
    let mut bytes = Vec::with_capacity(data.len() / 4 * 3);
    for chunk in data.as_bytes().as_chunks::<4>().0 {
        let a = value(chunk[0]).ok_or(Error::InvalidRequest)?;
        let b = value(chunk[1]).ok_or(Error::InvalidRequest)?;
        let c = if chunk[2] == b'=' { 0 } else { value(chunk[2]).ok_or(Error::InvalidRequest)? };
        let d = if chunk[3] == b'=' { 0 } else { value(chunk[3]).ok_or(Error::InvalidRequest)? };
        bytes.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            bytes.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            bytes.push((c << 6) | d);
        }
    }
    if crate::http::base64(&bytes) != data {
        return Err(Error::InvalidRequest);
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachments_enforce_canonical_base64_type_and_bounds() {
        let png = b"\x89PNG\r\n\x1a\nexample";
        let image = Image::from_bytes(png).unwrap();
        assert_eq!(image.validate().unwrap(), png.len());
        let mut invalid = image.clone();
        invalid.mime_type = "image/jpeg".into();
        assert_eq!(invalid.validate(), Err(Error::InvalidRequest));
        invalid = image.clone();
        invalid.data.push('=');
        assert_eq!(invalid.validate(), Err(Error::InvalidRequest));
        assert_eq!(Image::from_bytes(b"<svg/>").unwrap_err(), Error::InvalidRequest);
        assert_eq!(validate(&vec![image; MAX_IMAGES + 1]), Err(Error::Limit));
        let mut oversized = b"\x89PNG\r\n\x1a\n".to_vec();
        oversized.resize(MAX_IMAGE_BYTES + 1, 0);
        assert_eq!(Image::from_bytes(&oversized).unwrap_err(), Error::Limit);
        oversized.truncate(MAX_IMAGE_BYTES);
        let bounded = Image::from_bytes(&oversized).unwrap();
        assert_eq!(bounded.validate(), Ok(MAX_IMAGE_BYTES));
        assert_eq!(validate(&[bounded.clone(), bounded]), Err(Error::Limit));
    }
}
