//! Length-prefixed MessagePack framing: `[u32 LE length][rmp payload]`.

use crate::protocol::MAX_FRAME_LEN;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn invalid_data<E>(e: E) -> io::Error
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Refuse any frame over `MAX_FRAME_LEN` before it is written or allocated —
/// the cap is the only thing standing between a corrupt header and a
/// multi-gigabyte `vec![0u8; len]`.
fn check_len(len: u32) -> io::Result<()> {
    if len > MAX_FRAME_LEN {
        return Err(invalid_data("frame too large"));
    }
    Ok(())
}

pub async fn write_frame<T, W>(w: &mut W, msg: &T) -> io::Result<()>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let payload = rmp_serde::to_vec(msg).map_err(invalid_data)?;
    let len = payload.len() as u32;
    check_len(len)?;
    w.write_all(&len.to_le_bytes()).await?;
    w.write_all(&payload).await?;
    w.flush().await
}

/// One frame off the wire, as [`read_frame_or_undecodable`] hands it over.
#[derive(Debug, PartialEq)]
pub enum Frame<T> {
    Msg(T),
    /// The frame arrived whole, but its payload does not decode as `T`.
    /// The stream still sits on a frame boundary, so the reader can answer
    /// and read on rather than drop the connection.
    Undecodable {
        error: String,
        /// The payload's first field when it is an integer: the `req_id`
        /// every request that gets an Ack or Error carries first, so the
        /// refusal can reach the intent waiting on it.
        req_id: Option<u64>,
    },
}

/// The payload of the next frame. Returns Ok(None) on clean EOF at a frame
/// boundary.
async fn read_payload<R>(r: &mut R) -> io::Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf);
    check_len(len)?;
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

/// Returns Ok(None) on clean EOF at a frame boundary.
pub async fn read_frame<T, R>(r: &mut R) -> io::Result<Option<T>>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin,
{
    match read_payload(r).await? {
        Some(payload) => rmp_serde::from_slice(&payload)
            .map(Some)
            .map_err(invalid_data),
        None => Ok(None),
    }
}

/// [`read_frame`] for a reader that outlives a payload it can't decode: the
/// payload comes back as [`Frame::Undecodable`] instead of an error, so one
/// message from a skewed build costs that message, not the connection.
/// Transport errors and oversized frames are still errors — after those the
/// stream is no longer on a frame boundary.
pub async fn read_frame_or_undecodable<T, R>(r: &mut R) -> io::Result<Option<Frame<T>>>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin,
{
    let Some(payload) = read_payload(r).await? else {
        return Ok(None);
    };
    Ok(Some(match rmp_serde::from_slice(&payload) {
        Ok(msg) => Frame::Msg(msg),
        Err(error) => Frame::Undecodable {
            error: error.to_string(),
            req_id: leading_int(&payload),
        },
    }))
}

/// The leading field of an externally tagged enum variant encoded as
/// `{variant: [first, …]}`, when it is an unsigned integer — read without
/// knowing the variant's shape, so it works on a payload whose variant
/// doesn't decode.
fn leading_int(payload: &[u8]) -> Option<u64> {
    #[derive(serde::Deserialize, PartialEq, Eq, Hash)]
    #[serde(untagged)]
    enum Variant {
        Name(String),
        Index(u64),
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Field {
        Int(u64),
        Other(serde::de::IgnoredAny),
    }
    let tagged: std::collections::HashMap<Variant, Vec<Field>> =
        rmp_serde::from_slice(payload).ok()?;
    match tagged.into_values().next()?.into_iter().next()? {
        Field::Int(n) => Some(n),
        Field::Other(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &("hello".to_string(), 7u32))
            .await
            .unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let got: Option<(String, u32)> = read_frame(&mut cursor).await.unwrap();
        assert_eq!(got, Some(("hello".to_string(), 7)));
        let eof: Option<(String, u32)> = read_frame(&mut cursor).await.unwrap();
        assert_eq!(eof, None, "clean EOF at a frame boundary is Ok(None)");
    }

    #[tokio::test]
    async fn oversized_frames_are_refused_on_both_sides() {
        let big = vec![0u8; MAX_FRAME_LEN as usize + 1];
        let mut sink = Vec::new();
        let err = write_frame(&mut sink, &serde_bytes::ByteBuf::from(big))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("frame too large"), "got: {err}");
        assert!(sink.is_empty(), "nothing is written for a refused frame");

        let header = (MAX_FRAME_LEN + 1).to_le_bytes();
        let mut cursor = std::io::Cursor::new(header.to_vec());
        let err = read_frame::<Vec<u8>, _>(&mut cursor).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("frame too large"), "got: {err}");
    }

    /// The request enum as an older build knows it.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum OldRequest {
        Create { req_id: u64, prompt: String },
        Ping,
    }

    /// The same enum after a newer build gave `Create` a field without
    /// bumping `PROTOCOL_VERSION` — what bricked the TUI: the old reader
    /// failed the whole connection on it.
    #[derive(serde::Serialize)]
    enum NewRequest {
        Create {
            req_id: u64,
            prompt: String,
            mode: u8,
        },
    }

    #[tokio::test]
    async fn a_frame_from_a_skewed_build_costs_the_frame_not_the_stream() {
        let mut buf = Vec::new();
        let skewed = NewRequest::Create {
            req_id: 42,
            prompt: "fix the login".into(),
            mode: 1,
        };
        write_frame(&mut buf, &skewed).await.unwrap();
        write_frame(&mut buf, &OldRequest::Ping).await.unwrap();
        let mut cursor = std::io::Cursor::new(buf);

        let strict = read_frame::<OldRequest, _>(&mut cursor.clone()).await;
        assert!(strict.is_err(), "the strict reader still refuses it");

        match read_frame_or_undecodable::<OldRequest, _>(&mut cursor).await {
            Ok(Some(Frame::Undecodable { req_id, error })) => {
                assert_eq!(req_id, Some(42), "the refusal can reach its intent");
                assert!(!error.is_empty());
            }
            other => panic!("expected an undecodable frame, got {other:?}"),
        }
        assert_eq!(
            read_frame_or_undecodable::<OldRequest, _>(&mut cursor)
                .await
                .unwrap(),
            Some(Frame::Msg(OldRequest::Ping)),
            "the next frame still reads"
        );
        assert_eq!(
            read_frame_or_undecodable::<OldRequest, _>(&mut cursor)
                .await
                .unwrap(),
            None
        );
    }

    #[test]
    fn only_a_leading_unsigned_integer_is_taken_for_a_req_id() {
        let create = rmp_serde::to_vec(&OldRequest::Create {
            req_id: 7,
            prompt: "x".into(),
        })
        .unwrap();
        assert_eq!(leading_int(&create), Some(7));
        let unit = rmp_serde::to_vec(&OldRequest::Ping).unwrap();
        assert_eq!(leading_int(&unit), None, "a unit variant has no fields");
        #[derive(serde::Serialize)]
        enum Named {
            Input { session: String, data: u64 },
        }
        let named = rmp_serde::to_vec(&Named::Input {
            session: "a".into(),
            data: 3,
        })
        .unwrap();
        assert_eq!(
            leading_int(&named),
            None,
            "a string first field is no req_id"
        );
        assert_eq!(leading_int(b"\xc1"), None, "garbage is no req_id");
    }
}
