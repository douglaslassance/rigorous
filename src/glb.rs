//! Self-contained GLB I/O that keeps the JSON verbatim and only appends binary data.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

const MAGIC: u32 = 0x4654_6C67;
const VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;
const FLOAT: u32 = 5126;

pub struct Glb {
    pub json: Value,
    pub bin: Vec<u8>,
}

/// Accessor data decoded to floats, `width` components per element.
pub struct Accessor {
    pub values: Vec<f32>,
    pub width: usize,
}

impl Glb {
    pub fn read(path: &Path) -> Result<Self> {
        let data = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        Self::parse(&data).with_context(|| format!("cannot parse {}", path.display()))
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 12 || read_u32(data, 0) != MAGIC {
            bail!("not a GLB file");
        }
        let version = read_u32(data, 4);
        if version != VERSION {
            bail!("unsupported GLB version {version}");
        }
        let total = (read_u32(data, 8) as usize).min(data.len());
        let mut offset = 12;
        let mut json = None;
        let mut bin = None;
        while offset + 8 <= total {
            let length = read_u32(data, offset) as usize;
            let kind = read_u32(data, offset + 4);
            let start = offset + 8;
            let end = start
                .checked_add(length)
                .filter(|&end| end <= total)
                .context("truncated chunk")?;
            match kind {
                CHUNK_JSON if json.is_none() => {
                    json = Some(serde_json::from_slice(&data[start..end])?)
                }
                CHUNK_BIN if bin.is_none() => bin = Some(data[start..end].to_vec()),
                _ => {}
            }
            offset = end;
        }
        let json: Value = json.context("missing JSON chunk")?;
        if !json.is_object() {
            bail!("JSON chunk is not an object");
        }
        Ok(Self {
            json,
            bin: bin.unwrap_or_default(),
        })
    }

    pub fn to_bytes(&mut self) -> Result<Vec<u8>> {
        if !self.bin.is_empty() {
            self.ensure_bin_buffer()?;
            let length = self.bin.len();
            self.array_mut("buffers")[0]["byteLength"] = json!(length);
        }
        let mut json_bytes = serde_json::to_vec(&self.json)?;
        pad(&mut json_bytes, b' ');
        let mut bin = self.bin.clone();
        pad(&mut bin, 0);
        let bin_chunk_size = if bin.is_empty() { 0 } else { 8 + bin.len() };
        let total = 12 + 8 + json_bytes.len() + bin_chunk_size;
        let mut bytes = Vec::with_capacity(total);
        bytes.extend(MAGIC.to_le_bytes());
        bytes.extend(VERSION.to_le_bytes());
        bytes.extend(u32::try_from(total)?.to_le_bytes());
        push_chunk(&mut bytes, CHUNK_JSON, &json_bytes);
        if !bin.is_empty() {
            push_chunk(&mut bytes, CHUNK_BIN, &bin);
        }
        Ok(bytes)
    }

    /// Write through a temporary file so a failure never leaves a half written GLB.
    pub fn write(&mut self, path: &Path) -> Result<()> {
        let bytes = self.to_bytes()?;
        let temporary = path.with_extension("glb.tmp");
        fs::write(&temporary, bytes)
            .with_context(|| format!("cannot write {}", temporary.display()))?;
        fs::rename(&temporary, path).with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }

    pub fn array(&self, key: &str) -> &[Value] {
        self.json
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn array_mut(&mut self, key: &str) -> &mut Vec<Value> {
        let object = self
            .json
            .as_object_mut()
            .expect("GLB JSON root is an object");
        let entry = object
            .entry(key)
            .or_insert_with(|| Value::Array(Vec::new()));
        if !entry.is_array() {
            *entry = Value::Array(Vec::new());
        }
        entry.as_array_mut().expect("entry was just made an array")
    }

    pub fn read_accessor(&self, index: usize) -> Result<Accessor> {
        let accessor = self
            .array("accessors")
            .get(index)
            .with_context(|| format!("accessor {index} does not exist"))?;
        let count = as_usize(accessor.get("count")).context("accessor has no count")?;
        let width = match accessor.get("type").and_then(Value::as_str) {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") | Some("MAT2") => 4,
            Some("MAT3") => 9,
            Some("MAT4") => 16,
            other => bail!("accessor {index} has unsupported type {other:?}"),
        };
        if accessor.get("sparse").is_some() {
            bail!("accessor {index} is sparse, which is not supported yet");
        }
        let component_type =
            as_usize(accessor.get("componentType")).context("accessor has no componentType")?;
        let component_size = match component_type {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            other => bail!("accessor {index} has unsupported component type {other}"),
        };
        let normalized = accessor
            .get("normalized")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(view_index) = as_usize(accessor.get("bufferView")) else {
            return Ok(Accessor {
                values: vec![0.0; count * width],
                width,
            });
        };
        let view = self
            .array("bufferViews")
            .get(view_index)
            .with_context(|| format!("buffer view {view_index} does not exist"))?;
        if view
            .pointer("/extensions/EXT_meshopt_compression")
            .is_some()
        {
            bail!("meshopt compressed data is not supported yet, decompress the file first");
        }
        if as_usize(view.get("buffer")).unwrap_or(0) != 0 {
            bail!("only data stored in the GLB binary chunk is supported");
        }
        let element_size = component_size * width;
        let stride = as_usize(view.get("byteStride"))
            .filter(|&stride| stride > 0)
            .unwrap_or(element_size);
        let base = as_usize(view.get("byteOffset")).unwrap_or(0)
            + as_usize(accessor.get("byteOffset")).unwrap_or(0);
        let mut values = Vec::with_capacity(count * width);
        for element in 0..count {
            let start = base + element * stride;
            for component in 0..width {
                let offset = start + component * component_size;
                let bytes = self
                    .bin
                    .get(offset..offset + component_size)
                    .with_context(|| {
                        format!("accessor {index} reads past the end of the binary chunk")
                    })?;
                values.push(decode(bytes, component_type, normalized));
            }
        }
        Ok(Accessor { values, width })
    }

    /// Append float data as a new buffer view and accessor, returning the accessor index.
    pub fn append_accessor(&mut self, values: &[f32], kind: &str, bounds: bool) -> Result<usize> {
        let width = match kind {
            "SCALAR" => 1,
            "VEC3" => 3,
            "VEC4" => 4,
            other => bail!("cannot append accessor of type {other}"),
        };
        self.ensure_bin_buffer()?;
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        for value in values {
            self.bin.extend(value.to_le_bytes());
        }
        let views = self.array_mut("bufferViews");
        views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": values.len() * 4}));
        let view = views.len() - 1;
        let mut accessor = json!({
            "bufferView": view,
            "componentType": FLOAT,
            "count": values.len() / width,
            "type": kind,
        });
        if bounds && !values.is_empty() {
            let mut minimum = vec![f32::INFINITY; width];
            let mut maximum = vec![f32::NEG_INFINITY; width];
            for element in values.chunks_exact(width) {
                for (component, &value) in element.iter().enumerate() {
                    minimum[component] = minimum[component].min(value);
                    maximum[component] = maximum[component].max(value);
                }
            }
            accessor["min"] = json!(minimum);
            accessor["max"] = json!(maximum);
        }
        let accessors = self.array_mut("accessors");
        accessors.push(accessor);
        Ok(accessors.len() - 1)
    }

    fn ensure_bin_buffer(&mut self) -> Result<()> {
        let buffers = self.array_mut("buffers");
        if buffers.is_empty() {
            buffers.push(json!({"byteLength": 0}));
        }
        if buffers[0].get("uri").is_some() {
            bail!(
                "buffer 0 points to an external file, only self-contained GLB files are supported"
            );
        }
        Ok(())
    }
}

pub fn as_usize(value: Option<&Value>) -> Option<usize> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().expect("four bytes"))
}

fn decode(bytes: &[u8], component_type: usize, normalized: bool) -> f32 {
    match component_type {
        5126 => f32::from_le_bytes(bytes.try_into().expect("four bytes")),
        5120 => {
            let value = bytes[0] as i8 as f32;
            if normalized {
                (value / 127.0).max(-1.0)
            } else {
                value
            }
        }
        5121 => {
            let value = bytes[0] as f32;
            if normalized { value / 255.0 } else { value }
        }
        5122 => {
            let value = i16::from_le_bytes([bytes[0], bytes[1]]) as f32;
            if normalized {
                (value / 32767.0).max(-1.0)
            } else {
                value
            }
        }
        5123 => {
            let value = u16::from_le_bytes([bytes[0], bytes[1]]) as f32;
            if normalized { value / 65535.0 } else { value }
        }
        _ => u32::from_le_bytes(bytes.try_into().expect("four bytes")) as f32,
    }
}

fn pad(bytes: &mut Vec<u8>, filler: u8) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(filler);
    }
}

fn push_chunk(bytes: &mut Vec<u8>, kind: u32, data: &[u8]) {
    bytes.extend(
        u32::try_from(data.len())
            .expect("chunk fits in u32")
            .to_le_bytes(),
    );
    bytes.extend(kind.to_le_bytes());
    bytes.extend(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Glb {
        Glb {
            json: json!({"asset": {"version": "2.0"}}),
            bin: Vec::new(),
        }
    }

    #[test]
    fn round_trip_keeps_json_and_appended_data() {
        let mut glb = empty();
        let index = glb
            .append_accessor(&[0.0, 0.5, 1.0], "SCALAR", true)
            .unwrap();
        let bytes = glb.to_bytes().unwrap();
        let parsed = Glb::parse(&bytes).unwrap();
        assert_eq!(parsed.json["asset"]["version"], "2.0");
        assert_eq!(parsed.json["accessors"][index]["max"], json!([1.0]));
        assert_eq!(
            parsed.read_accessor(index).unwrap().values,
            vec![0.0, 0.5, 1.0]
        );
        assert_eq!(bytes.len() % 4, 0);
    }

    #[test]
    fn reads_normalized_shorts_with_stride() {
        let mut glb = empty();
        glb.bin = [32767i16, -32768, 0, 0, 16384, 0, 0, 0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        glb.json["buffers"] = json!([{"byteLength": glb.bin.len()}]);
        glb.json["bufferViews"] = json!([{"buffer": 0, "byteLength": 16, "byteStride": 8}]);
        glb.json["accessors"] = json!([{"bufferView": 0, "componentType": 5122, "normalized": true, "count": 2, "type": "VEC2"}]);
        let accessor = glb.read_accessor(0).unwrap();
        assert_eq!(accessor.width, 2);
        assert!((accessor.values[0] - 1.0).abs() < 1e-6);
        assert!((accessor.values[1] + 1.0).abs() < 1e-6);
        assert!((accessor.values[2] - 0.5).abs() < 1e-4);
    }
}
