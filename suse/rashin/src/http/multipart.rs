//! Narrow multipart parser for the authenticated conversation image form.
//! Filenames and caller MIME headers are ignored; raster type comes from bytes.
use crate::{Error, Result, api::Request, images::Image, runtime::Engine};
use std::{collections::BTreeMap, sync::Arc};

fn position(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|window| window == needle)
}
pub(super) fn submit(engine: &Arc<Engine>, body: &[u8], boundary: &str) -> Result<String> {
    let boundary = boundary.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(boundary);
    if boundary.is_empty()
        || boundary.len() > 70
        || !boundary.bytes().all(|b| b.is_ascii_alphanumeric() || b"'()+_,-./:=?".contains(&b))
    {
        return Err(Error::InvalidRequest);
    }
    let first = format!("--{boundary}\r\n");
    let delimiter = format!("\r\n--{boundary}");
    let mut rest = body.strip_prefix(first.as_bytes()).ok_or(Error::InvalidRequest)?;
    let mut fields = BTreeMap::new();
    let mut images = Vec::new();
    let mut parts = 0;
    loop {
        parts += 1;
        if parts > 8 {
            return Err(Error::Limit);
        }
        let end =
            position(rest, b"\r\n\r\n").filter(|end| *end <= 8192).ok_or(Error::InvalidRequest)?;
        let header = std::str::from_utf8(&rest[..end]).map_err(|_| Error::InvalidRequest)?;
        let mut disposition = None;
        let mut content_type = false;
        for line in header.split("\r\n") {
            let (name, value) = line.split_once(':').ok_or(Error::InvalidRequest)?;
            if value.bytes().any(|b| b.is_ascii_control() && b != b'\t') {
                return Err(Error::InvalidRequest);
            }
            if name.eq_ignore_ascii_case("content-disposition") {
                if disposition.replace(value.trim()).is_some() {
                    return Err(Error::InvalidRequest);
                }
            } else if name.eq_ignore_ascii_case("content-type") {
                if content_type {
                    return Err(Error::InvalidRequest);
                }
                content_type = true;
            } else {
                return Err(Error::InvalidRequest);
            }
        }
        let disposition = disposition
            .ok_or(Error::InvalidRequest)?
            .strip_prefix("form-data; name=\"")
            .ok_or(Error::InvalidRequest)?;
        let (name, tail) = disposition.split_once('"').ok_or(Error::InvalidRequest)?;
        let file = tail.starts_with("; filename=\"") && tail.ends_with('"');
        if !tail.is_empty() && !file {
            return Err(Error::InvalidRequest);
        }
        rest = &rest[end + 4..];
        let end = position(rest, delimiter.as_bytes()).ok_or(Error::InvalidRequest)?;
        let value = &rest[..end];
        rest = &rest[end + delimiter.len()..];
        if name == "images" && file {
            if !value.is_empty() {
                images.push(Image::from_bytes(value)?);
            }
        } else {
            if file
                || !matches!(name, "action" | "id" | "session" | "prompt")
                || value.len() > 16 * 1024
            {
                return Err(Error::InvalidRequest);
            }
            let value = std::str::from_utf8(value).map_err(|_| Error::InvalidRequest)?.to_owned();
            if fields.insert(name.to_owned(), value).is_some() {
                return Err(Error::InvalidRequest);
            }
        }
        if rest == b"--" || rest == b"--\r\n" {
            break;
        }
        rest = rest.strip_prefix(b"\r\n").ok_or(Error::InvalidRequest)?;
    }
    crate::images::validate(&images)?;
    if fields.get("action").map(String::as_str) != Some("prompt") {
        return Err(Error::InvalidRequest);
    }
    let id = fields.remove("id").ok_or(Error::InvalidRequest)?;
    let session = fields.remove("session").ok_or(Error::InvalidRequest)?;
    let prompt = fields.remove("prompt").ok_or(Error::InvalidRequest)?;
    if images.is_empty() {
        engine.dispatch(Request::Prompt { id: id.clone(), session, prompt })?;
    } else {
        engine.dispatch(Request::PromptImages { id: id.clone(), session, prompt, images })?;
    }
    crate::identifier(&id)?;
    Ok(format!("/job/{id}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn malformed_boundaries_duplicate_fields_and_non_raster_uploads_are_rejected() {
        let root = std::env::temp_dir().join(format!("rashin-multipart-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let engine = Engine::open(Store::open(&root).unwrap()).unwrap();
        assert_eq!(submit(&engine, b"anything", "bad\r\nboundary"), Err(Error::InvalidRequest));
        assert_eq!(submit(&engine,b"--b\r\nContent-Disposition: form-data; name=\"action\"\r\n\r\nprompt\r\n--b\r\nContent-Disposition: form-data; name=\"action\"\r\n\r\nprompt\r\n--b--\r\n","b"),Err(Error::InvalidRequest));
        assert_eq!(submit(&engine,b"--b\r\nContent-Disposition: form-data; name=\"images\"; filename=\"x.svg\"\r\nContent-Type: image/png\r\n\r\n<svg/>\r\n--b--\r\n","b"),Err(Error::InvalidRequest));
        assert_eq!(
            submit(
                &engine,
                b"--b\r\nContent-Disposition: form-data; name=\"action\"\r\n\r\nprompt",
                "b"
            ),
            Err(Error::InvalidRequest)
        );
        assert!(engine.dispatch(Request::Jobs {}).unwrap()["jobs"].as_array().unwrap().is_empty());
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
}
