//! Equivalent encodings of one selected canonical graph, with exact decoding.
use crate::{Error, Result};
use serde_json::{Map, Value as J};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ViewFormat {
    #[default]
    Json,
    CheckedText,
    CheckedTextRows,
    CheckedTextTagged,
}
impl ViewFormat {
    pub fn name(self) -> &'static str {
        match self { Self::Json => "json", Self::CheckedText => "checked-text", Self::CheckedTextRows => "checked-text-rows", Self::CheckedTextTagged => "checked-text-tagged" }
    }
}
const SECTIONS: [&str;4] = ["nodes","groups","links","expanded_edges"];
fn row_tag(section: &str) -> &'static str {
    match section { "nodes"=>"node", "groups"=>"group", "links"=>"link", "expanded_edges"=>"edge", _=>unreachable!() }
}
fn header(version: u8) -> String {
    format!("# Checked graph view (kpopper.view-codec/v{version}; {})",crate::canonical_view::COMPACT_SCHEMA)
}
pub fn render(packet: &J, format: ViewFormat) -> Result<String> {
    if format == ViewFormat::Json { return Ok(serde_json::to_string(packet)?+"\n"); }
    crate::require(packet["schema"] == crate::canonical_view::COMPACT_SCHEMA,"checked text needs the current selected graph schema")?;
    let tagged=format==ViewFormat::CheckedTextTagged;
    let rows_format=tagged || format==ViewFormat::CheckedTextRows;
    let mut lines=vec![header(if tagged {4} else if rows_format {3} else {2})];
    for (key,value) in packet.as_object().ok_or_else(||Error("graph must be an object".into()))? {
        let key_json=serde_json::to_string(key)?;
        if SECTIONS.contains(&key.as_str()) {
            let rows=value.as_array().ok_or_else(||Error("graph section must be an array".into()))?;
            lines.push(format!("\n## {key}"));
            lines.push(if rows_format {format!("@rows {key_json} {}",rows.len())} else {format!("@section {key_json}")});
            for (index,row) in rows.iter().enumerate() {
                let data=serde_json::to_string(row)?;
                lines.push(if tagged {format!("{} {data}",row_tag(key))} else if rows_format {data} else {format!("@item {key_json} {index} {data}")});
            }
        } else { lines.push(format!("@field {key_json} {}",serde_json::to_string(value)?)); }
    }
    Ok(lines.join("\n")+"\n")
}
pub fn decode_checked_text(text: &str) -> Result<J> {
    let mut lines=text.lines();
    let first=lines.next().unwrap_or_default();let tagged=first==header(4);
    let rows_format=tagged || first==header(3);
    crate::require(rows_format || first==header(2),"unsupported checked graph header")?;
    let mut fields=Map::new();
    let mut section:Option<(String,usize)>=None;
    for line in lines {
        if line.is_empty() || line.starts_with("## ") { continue; }
        if rows_format && !line.starts_with('@') {
            let (key,count)=section.as_ref().ok_or_else(||Error("graph row lacks its section".into()))?;
            let data=if tagged {line.strip_prefix(&format!("{} ",row_tag(key)))
                .ok_or_else(||Error("graph row kind disagrees with its section".into()))?} else {line};
            let rows=fields.get_mut(key).and_then(J::as_array_mut).ok_or_else(||Error("invalid graph section".into()))?;
            crate::require(rows.len()<*count,"extra graph row")?;rows.push(serde_json::from_str(data)?);continue;
        }
        if let Some((key,count))=&section {
            crate::require(fields[key].as_array().is_some_and(|r|r.len()==*count),"missing graph row")?;
        }
        section=None;
        let (kind,rest)=line.split_once(' ').ok_or_else(||Error("invalid checked graph row".into()))?;
        let values=serde_json::Deserializer::from_str(rest).into_iter::<J>().collect::<std::result::Result<Vec<_>,_>>()?;
        let key=values.first().and_then(J::as_str).ok_or_else(||Error("checked graph row lacks a field name".into()))?;
        match kind {
            "@rows" if rows_format => {
                crate::require(values.len()==2 && SECTIONS.contains(&key) && !fields.contains_key(key),"invalid or duplicate graph rows")?;
                let count=values[1].as_u64().ok_or_else(||Error("invalid graph row count".into()))? as usize;
                fields.insert(key.into(),J::Array(vec![]));section=Some((key.into(),count));
            },
            "@field" => {
                crate::require(values.len()==2 && !SECTIONS.contains(&key) && !fields.contains_key(key),"invalid or duplicate graph field")?;
                fields.insert(key.into(),values[1].clone());
            },
            "@section" if !rows_format => {
                crate::require(values.len()==1 && SECTIONS.contains(&key) && !fields.contains_key(key),"invalid or duplicate graph section")?;
                fields.insert(key.into(),J::Array(vec![]));
            },
            "@item" if !rows_format => {
                crate::require(values.len()==3,"invalid graph item")?;
                let rows=fields.get_mut(key).and_then(J::as_array_mut).ok_or_else(||Error("graph item lacks its section".into()))?;
                crate::require(values[1].as_u64()==Some(rows.len() as u64),"missing or duplicate graph item")?;
                rows.push(values[2].clone());
            },
            _=>return Err(Error("unknown checked graph row".into())),
        }
    }
    if let Some((key,count))=section {crate::require(fields[&key].as_array().is_some_and(|r|r.len()==count),"missing final graph row")?;}
    let packet=J::Object(fields);
    crate::require(packet["schema"]==crate::canonical_view::COMPACT_SCHEMA && SECTIONS.iter().all(|s|packet[*s].is_array()),"incomplete checked graph")?;
    for key in ["project","revision","scope","mode","view_id","rules"] {
        crate::require(packet[key].is_string(),"incomplete checked graph envelope")?;
    }
    for key in ["dictionary","fields","coverage","navigation_facets","descriptions"] {
        crate::require(packet[key].is_object(),"incomplete checked graph metadata")?;
    }
    crate::require(packet["project_identity"].is_array()
        && packet["edge_set_handle_template"].is_string()
        && packet["group_expansion_route"].is_string(),"incomplete checked graph identity or routes")?;
    Ok(packet)
}
