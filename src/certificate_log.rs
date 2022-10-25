use std::io::{Cursor};

use byteorder::{
    
    ReadBytesExt,
    BigEndian, 
};

use chrono::{

    NaiveDateTime, 
    DateTime, 
    Utc,
};

use serde::{

    Deserialize, 
    Serialize,
};

use crate::{
    
    certificate::{Certificate}, 
    certificate,

    error::{
        
        StreamError,
        LogError,
    },
};

use std::time::{Duration};

use reqwest::{

    blocking::{Client},
    Url, 
};

#[derive(Debug, Deserialize)]
struct Tree {
    tree_size: usize
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    leaf_input: String
}

#[derive(Debug, Deserialize)]
struct TreeResponse {
    entries: Vec<TreeEntry>
}

#[derive(Debug, Clone)]
#[derive(Serialize, Deserialize)]
pub enum Entry {

    Signed {

        timestamp: DateTime<Utc>,
        certificate: Certificate,
    },

    Pending {

        timestamp: DateTime<Utc>,
        certificate: Certificate,
    },
}

impl<'a> Entry {
    pub fn timestamp(&'a self) -> DateTime<Utc> {
        match self {

            Entry::Signed { ref timestamp, .. } => timestamp.clone(),
            Entry::Pending { ref timestamp, .. } => timestamp.clone(),
        }
    }

    pub fn certificate(&'a self) -> &'a Certificate {
        match self {

            Entry::Signed { ref certificate, .. } => certificate,
            Entry::Pending { ref certificate, .. } => certificate,
        }
    }
}

fn parse_log_entry(data: &[u8]) -> Result<Entry, LogError> {
    let mut cursor = Cursor::new(data);

    let leaf_version = cursor.read_u8()
        .map_err(|_| LogError::Parse("failed to read leaf version!"))?;

    let leaf_variant = cursor.read_u8()
        .map_err(|_| LogError::Parse("failed to read leaf variant!"))?;

    let timestamp = {

        let raw = cursor.read_u64::<BigEndian>()
            .map_err(|_| LogError::Parse("failed to read leaf timestamp!"))?;

        NaiveDateTime::from_timestamp((raw / 1000) as i64, 0)
    };

    let leaf_entry_variant = cursor.read_u16::<BigEndian>()
        .map_err(|_| LogError::Parse("failed to read leaf entry variant!"))?;

    match leaf_version {
        0 => match leaf_variant {
            0 => {

                match leaf_entry_variant {
                    0 => cursor.set_position(cursor.position()),
                    1 => cursor.set_position(cursor.position() + 32),
                    _ => return Err(LogError::UnsupportedEntry({
                        leaf_entry_variant
                    })),
                };

                let length = cursor.read_u24::<BigEndian>()
                    .map_err(|_| LogError::Parse("failed to read certificate length!"))?;

                let start = cursor.position() as usize;
                let end = start + length as usize;

                let certificate = certificate::parse_certificate(data[start..end].as_ref())
                    .ok_or(LogError::Parse("failed to parse certificate!"))?;
                
                Ok(match leaf_entry_variant {
                    0 => Entry::Signed {
                        timestamp: DateTime::from_utc(timestamp, Utc),
                        certificate,
                    },
                    1 => Entry::Pending { 
                        timestamp: DateTime::from_utc(timestamp, Utc), 
                        certificate,
                    },
                    _ => return Err(LogError::UnsupportedEntry(leaf_entry_variant)),
                })
            },
            _ => Err(LogError::UnsupportedLeaf(leaf_variant)),
        },
        _ => Err(LogError::UnsupportedVersion(leaf_version)),
    }
}

fn parse_log_size<T: AsRef<str>>(text: T) -> Result<usize, LogError> {
    let Tree { tree_size } = serde_json::from_str(text.as_ref())
        .map_err(|_| LogError::Parse("invalid log response when getting tree size!"))?;

    Ok(tree_size)
}

fn parse_log_entries<T: AsRef<str>>(text: T) -> Result<Vec<Entry>, LogError> {
    let TreeResponse { entries } = serde_json::from_str(text.as_ref())
        .map_err(|_| LogError::Parse("invalid log response when getting entries!"))?;

    let mut processed = Vec::with_capacity({
        entries.len()
    });

    for TreeEntry { leaf_input } in entries {
        let data = base64::decode(leaf_input).map_err(|_| {
            LogError::Parse("when decoding leaf input!")
        })?;

        processed.push(self::parse_log_entry(data.as_slice())?);
    }

    Ok(processed)
}

pub(crate) fn get_log_size<E>(client: &Client, endpoint: E) -> Result<usize, StreamError> 
where E: AsRef<str> {

    let mut url = Url::parse(endpoint.as_ref()).map_err(|_| {
        StreamError::InvalidEndpoint
    })?;

    url.path_segments_mut().map_err(|_| {
        StreamError::InvalidEndpoint
    })?.push("/ct/v1/get-sth");

    let text = loop {

        let response = client.get(url.as_ref())
            .send().map_err(|_| StreamError::Connection("when requesting tree size!"))?;
        
        if response.status()
            .is_success() {

                break response.text()
                    .map_err(|_| StreamError::Response("when getting text!"))?
            }

        std::thread::sleep({
            Duration::from_millis(1000)
        });
    };



    Ok(self::parse_log_size(text).map_err(|error| {
        StreamError::Parse(error)
    })?)
}

pub(crate) fn get_log_entries<E>(client: &Client, endpoint: E, position: usize, count: usize) -> Result<Vec<Entry>, StreamError> 
where E: AsRef<str> {

    let mut url = Url::parse(endpoint.as_ref())
        .map_err(|_| StreamError::InvalidEndpoint)?;

    url.path_segments_mut()
        .map_err(|_| StreamError::InvalidEndpoint)?
        .push("/ct/v1/get-entries");

    let response = client.get(url.as_ref())
        .query([("start", position), ("end", position + count)].as_ref())
        .send().map_err(|_| StreamError::Connection("when requesting entries!"))?;

    if response.status()
        .is_success() {

            let text = response.text()
                .map_err(|_| StreamError::Response("when getting text!"))?;

            return Ok(self::parse_log_entries(text).map_err(|error| {
                StreamError::Parse(error)
            })?)
        }

    Ok(Vec::with_capacity(0))
}