//! YAML → [`Blueprint`], tolerant of the full Render schema.
//!
//! The YAML is read as a `serde_yaml::Value` tree and known keys are
//! extracted by hand, so unknown keys can be reported as warnings instead of
//! failing deserialization.
//!
//! Every non-null scalar is kept **exactly as written** (see [`load`]):
//! `PYTHON_VERSION: 3.10` stays `3.10` (not the float `3.1`), `0x1F` stays
//! `0x1F`, `True` stays `True`, and integers of any size are text. Fields
//! that need a number or a boolean parse that text themselves.

use std::fmt;
use std::str::FromStr;

use ferry_core::{DatastoreKind, Error, Result, Runtime, ServiceType, resources};
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_yaml::{Mapping, Sequence, Value};

use super::plans::{self, PlanTable};
use super::{Blueprint, DatastoreSpec, EnvGroupSpec, EnvVarSpec, LimitsSpec, RefTarget, ServiceSpec};

/// Parse a `ferry.yaml` / `render.yaml` document.
pub fn parse(yaml: &str) -> Result<Blueprint> {
    let mut root = load(yaml).map_err(|e| Error::invalid(format!("invalid blueprint YAML: {e}")))?;
    root.apply_merge().map_err(|e| Error::invalid(format!("invalid blueprint YAML (merge keys): {e}")))?;
    let mut bp = Blueprint::default();
    let map = match &root {
        Value::Null => {
            bp.warnings.push("the blueprint is empty".to_string());
            return Ok(bp);
        }
        Value::Mapping(m) => m,
        _ => {
            return Err(Error::invalid(
                "invalid blueprint: expected a YAML mapping with services, databases and/or envVarGroups",
            ));
        }
    };

    let mut groups: Vec<&Value> = Vec::new();
    let mut databases: Vec<&Value> = Vec::new();
    let mut services: Vec<&Value> = Vec::new();

    let mut top = Entry::new(map, "blueprint");
    groups.extend(top.seq("envVarGroups")?.into_iter().flatten());
    databases.extend(top.seq("databases")?.into_iter().flatten());
    services.extend(top.seq("services")?.into_iter().flatten());
    if let Some(projects) = top.seq("projects")? {
        bp.warnings.push(
            "projects/environments are not supported: resources of every environment are applied to this server"
                .to_string(),
        );
        for (pi, project) in projects.iter().enumerate() {
            let pctx = format!("projects[{pi}]");
            let pmap = as_mapping(project, &pctx)?;
            let mut p = Entry::new(pmap, &pctx);
            p.skip("name");
            for (ei, env) in p.seq("environments")?.into_iter().flatten().enumerate() {
                let ectx = format!("{pctx}.environments[{ei}]");
                let emap = as_mapping(env, &ectx)?;
                let mut e = Entry::new(emap, &ectx);
                e.skip("name");
                groups.extend(e.seq("envVarGroups")?.into_iter().flatten());
                databases.extend(e.seq("databases")?.into_iter().flatten());
                services.extend(e.seq("services")?.into_iter().flatten());
                e.finish(&mut bp.warnings);
            }
            p.finish(&mut bp.warnings);
        }
    }
    top.finish(&mut bp.warnings);

    for (i, g) in groups.into_iter().enumerate() {
        let spec = parse_env_group(g, i, &mut bp.warnings)?;
        bp.env_groups.push(spec);
    }
    for (i, d) in databases.into_iter().enumerate() {
        let spec = parse_database(d, i, &mut bp.warnings)?;
        bp.datastores.push(spec);
    }
    for (i, s) in services.into_iter().enumerate() {
        match parse_service(s, i, &mut bp.warnings)? {
            Parsed::Service(spec) => bp.services.push(*spec),
            Parsed::Datastore(spec) => bp.datastores.push(spec),
        }
    }
    if bp.env_groups.is_empty() && bp.datastores.is_empty() && bp.services.is_empty() {
        bp.warnings.push("the blueprint declares no services, databases or env groups".to_string());
    }
    Ok(bp)
}

// ---------------------------------------------------------------------------
// literal-preserving YAML loading

/// Parse `yaml` into a [`Value`] whose non-null scalars are all
/// [`Value::String`]s holding the scalar exactly as written.
///
/// `serde_yaml` resolves plain scalars while building a `Value` (`3.10` →
/// `3.1`, `0x1F` → `31`, integers above `u64::MAX` → error), and the original
/// text is gone after that. Its deserializer does hand out the raw text of a
/// scalar to `deserialize_str`, but a caller has to know up front that a node
/// is a scalar. Hence two passes over the document: the first records the
/// shape of every node, the second, guided by that shape, reads scalars as
/// strings. (Anchors and aliases are expanded identically by both passes.)
pub(crate) fn load(yaml: &str) -> std::result::Result<Value, serde_yaml::Error> {
    let shape = Shape::deserialize(serde_yaml::Deserializer::from_str(yaml))?;
    if matches!(shape, Shape::Null) {
        return Ok(Value::Null);
    }
    Guided(&shape).deserialize(serde_yaml::Deserializer::from_str(yaml))
}

/// The shape of a YAML node (first pass of [`load`]).
enum Shape {
    Null,
    Scalar,
    /// A node with a custom `!tag`: read as a plain `Value`.
    Tagged,
    Seq(Vec<Shape>),
    Map(Vec<(Shape, Shape)>),
}

impl<'de> Deserialize<'de> for Shape {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(ShapeVisitor)
    }
}

struct ShapeVisitor;

impl<'de> Visitor<'de> for ShapeVisitor {
    type Value = Shape;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<Shape, E> {
        Ok(Shape::Scalar)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> std::result::Result<Shape, E> {
        Ok(Shape::Scalar)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> std::result::Result<Shape, E> {
        Ok(Shape::Scalar)
    }
    fn visit_i128<E: de::Error>(self, _: i128) -> std::result::Result<Shape, E> {
        Ok(Shape::Scalar)
    }
    fn visit_u128<E: de::Error>(self, _: u128) -> std::result::Result<Shape, E> {
        Ok(Shape::Scalar)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Shape, E> {
        Ok(Shape::Scalar)
    }
    fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<Shape, E> {
        Ok(Shape::Scalar)
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Shape, E> {
        Ok(Shape::Null)
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Shape, E> {
        Ok(Shape::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> std::result::Result<Shape, D::Error> {
        Shape::deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Shape, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<Shape>()? {
            items.push(item);
        }
        Ok(Shape::Seq(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Shape, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry::<Shape, Shape>()? {
            entries.push(entry);
        }
        Ok(Shape::Map(entries))
    }
    fn visit_enum<A: de::EnumAccess<'de>>(self, data: A) -> std::result::Result<Shape, A::Error> {
        use de::VariantAccess;
        let (_tag, contents): (IgnoredAny, _) = data.variant()?;
        contents.newtype_variant::<IgnoredAny>()?;
        Ok(Shape::Tagged)
    }
}

/// Second pass of [`load`]: read the node described by the shape.
struct Guided<'s>(&'s Shape);

impl<'de> DeserializeSeed<'de> for Guided<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> std::result::Result<Value, D::Error> {
        match self.0 {
            Shape::Null => {
                IgnoredAny::deserialize(d)?;
                Ok(Value::Null)
            }
            // `deserialize_str` hands out the scalar's source text.
            Shape::Scalar => Ok(Value::String(String::deserialize(d)?)),
            Shape::Tagged => Value::deserialize(d),
            Shape::Seq(items) => d.deserialize_seq(GuidedSeq(items)),
            Shape::Map(entries) => d.deserialize_map(GuidedMap(entries)),
        }
    }
}

struct GuidedSeq<'s>(&'s [Shape]);

impl<'de> Visitor<'de> for GuidedSeq<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a sequence")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut out = Sequence::with_capacity(self.0.len());
        for shape in self.0 {
            let item = seq
                .next_element_seed(Guided(shape))?
                .ok_or_else(|| de::Error::custom("the sequence changed length"))?;
            out.push(item);
        }
        Ok(Value::Sequence(out))
    }
}

struct GuidedMap<'s>(&'s [(Shape, Shape)]);

impl<'de> Visitor<'de> for GuidedMap<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a mapping")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut out = Mapping::with_capacity(self.0.len());
        for (key_shape, value_shape) in self.0 {
            let key =
                map.next_key_seed(Guided(key_shape))?.ok_or_else(|| de::Error::custom("the mapping changed length"))?;
            let value = map.next_value_seed(Guided(value_shape))?;
            if out.contains_key(&key) {
                let shown = key.as_str().map_or_else(|| format!("{key:?}"), |k| format!("\"{k}\""));
                return Err(de::Error::custom(format!("duplicate entry with key {shown}")));
            }
            out.insert(key, value);
        }
        Ok(Value::Mapping(out))
    }
}

// ---------------------------------------------------------------------------
// helpers

fn as_mapping<'a>(v: &'a Value, ctx: &str) -> Result<&'a Mapping> {
    match v {
        Value::Mapping(m) => Ok(m),
        Value::Tagged(t) => as_mapping(&t.value, ctx),
        _ => Err(Error::invalid(format!("{ctx}: expected a mapping"))),
    }
}

/// Render a scalar as a string. (After [`load`] every non-null scalar is
/// already its literal text; numbers and booleans only appear in `!tagged`
/// nodes.)
fn scalar_string(v: &Value) -> Option<Option<String>> {
    match v {
        Value::Null => Some(None),
        Value::String(s) => Some(Some(s.clone())),
        Value::Number(n) => Some(Some(n.to_string())),
        Value::Bool(b) => Some(Some(b.to_string())),
        Value::Tagged(t) => scalar_string(&t.value),
        Value::Sequence(_) | Value::Mapping(_) => None,
    }
}

/// A mapping whose keys are consumed one by one; leftovers become warnings.
struct Entry<'a> {
    map: &'a Mapping,
    ctx: String,
    used: Vec<String>,
}

impl<'a> Entry<'a> {
    fn new(map: &'a Mapping, ctx: &str) -> Self {
        Entry { map, ctx: ctx.to_string(), used: Vec::new() }
    }

    fn ctx(&self) -> &str {
        &self.ctx
    }

    /// Mark `key` as known without reading it.
    fn skip(&mut self, key: &str) {
        self.used.push(key.to_string());
    }

    /// The value of `key` (`None` when missing or null).
    fn get(&mut self, key: &str) -> Option<&'a Value> {
        self.used.push(key.to_string());
        match self.map.get(key) {
            None | Some(Value::Null) => None,
            Some(v) => Some(v),
        }
    }

    fn has(&self, key: &str) -> bool {
        matches!(self.map.get(key), Some(v) if !v.is_null())
    }

    fn string(&mut self, key: &str) -> Result<Option<String>> {
        let ctx = self.ctx.clone();
        match self.get(key) {
            None => Ok(None),
            Some(v) => scalar_string(v).ok_or_else(|| Error::invalid(format!("{ctx}: '{key}' must be a string"))),
        }
    }

    fn required_string(&mut self, key: &str) -> Result<String> {
        match self.string(key)? {
            Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
            _ => Err(Error::invalid(format!("{}: '{key}' is required", self.ctx))),
        }
    }

    fn bool(&mut self, key: &str) -> Result<Option<bool>> {
        let ctx = self.ctx.clone();
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(v) => {
                let s = scalar_string(v).flatten().unwrap_or_default();
                match s.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" | "on" | "1" => Ok(Some(true)),
                    "false" | "no" | "off" | "0" => Ok(Some(false)),
                    _ => Err(Error::invalid(format!("{ctx}: '{key}' must be true or false"))),
                }
            }
        }
    }

    fn number<T: FromStr>(&mut self, key: &str) -> Result<Option<T>> {
        let ctx = self.ctx.clone();
        match self.get(key) {
            None => Ok(None),
            Some(v) => {
                let s = scalar_string(v).flatten().unwrap_or_default();
                s.trim()
                    .parse::<T>()
                    .map(Some)
                    .map_err(|_| Error::invalid(format!("{ctx}: '{key}' must be a non-negative integer (got '{s}')")))
            }
        }
    }

    /// A TCP port: 1–65535.
    fn port(&mut self, key: &str) -> Result<Option<u16>> {
        let ctx = self.ctx.clone();
        match self.get(key) {
            None => Ok(None),
            Some(v) => {
                let s = scalar_string(v).flatten().unwrap_or_default();
                match s.trim().parse::<u16>() {
                    Ok(p) if p > 0 => Ok(Some(p)),
                    _ => Err(Error::invalid(format!("{ctx}: '{key}' must be a port between 1 and 65535 (got '{s}')"))),
                }
            }
        }
    }

    fn seq(&mut self, key: &str) -> Result<Option<&'a Sequence>> {
        let ctx = self.ctx.clone();
        match self.get(key) {
            None => Ok(None),
            Some(Value::Sequence(s)) => Ok(Some(s)),
            Some(_) => Err(Error::invalid(format!("{ctx}: '{key}' must be a list"))),
        }
    }

    fn mapping(&mut self, key: &str) -> Result<Option<&'a Mapping>> {
        let ctx = self.ctx.clone();
        match self.get(key) {
            None => Ok(None),
            Some(v) => as_mapping(v, &format!("{ctx}: '{key}'")).map(Some),
        }
    }

    fn string_list(&mut self, key: &str) -> Result<Option<Vec<String>>> {
        let ctx = self.ctx.clone();
        let Some(items) = self.seq(key)? else { return Ok(None) };
        let mut out = Vec::new();
        for item in items {
            match scalar_string(item) {
                Some(Some(s)) => out.push(s),
                Some(None) => {}
                None => return Err(Error::invalid(format!("{ctx}: '{key}' must be a list of strings"))),
            }
        }
        Ok(Some(out))
    }

    /// `plan` (looked up in `table`; `None` = the entry has no plans and the
    /// key is ignored), overridden by `memoryLimit` / `cpuLimit`. An unknown
    /// plan is a warning; a malformed limit is an error.
    fn limits(&mut self, table: Option<&PlanTable>, warnings: &mut Vec<String>) -> Result<LimitsSpec> {
        let ctx = self.ctx.clone();
        let mut out = LimitsSpec::default();
        match table {
            Some(table) => {
                if let Some(plan) = opt_trim(self.string("plan")?) {
                    match table.limits(&plan) {
                        Some(l) => {
                            out.memory_mb = Some(l.memory_mb);
                            out.cpus = l.cpus;
                        }
                        None => warnings.push(format!(
                            "{ctx}: ignoring unknown plan '{plan}' (known plans: {}); set memoryLimit / cpuLimit instead",
                            table.known
                        )),
                    }
                }
            }
            None => self.skip("plan"),
        }
        let invalid = |key: &str, e: Error| match e {
            Error::Invalid(m) => Error::Invalid(format!("{ctx}: '{key}': {m}")),
            other => other,
        };
        if let Some(raw) = opt_trim(self.string("memoryLimit")?) {
            out.memory_mb = Some(resources::parse_memory_mb(&raw).map_err(|e| invalid("memoryLimit", e))?);
        }
        if let Some(raw) = opt_trim(self.string("cpuLimit")?) {
            out.cpus = Some(resources::parse_cpus(&raw).map_err(|e| invalid("cpuLimit", e))?);
        }
        Ok(out)
    }

    /// Report every key that wasn't read.
    fn finish(self, warnings: &mut Vec<String>) {
        for k in self.map.keys() {
            match k.as_str() {
                Some(k) if self.used.iter().any(|u| u == k) => {}
                Some(k) => warnings.push(format!("{}: ignoring unsupported key '{k}'", self.ctx)),
                None => warnings.push(format!("{}: ignoring non-string key", self.ctx)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// env vars

enum EnvItem {
    Var(EnvVarSpec),
    Group(String),
}

fn parse_env_vars(
    list: Option<&Sequence>,
    ctx: &str,
    allow_groups: bool,
    warnings: &mut Vec<String>,
) -> Result<(Vec<EnvVarSpec>, Vec<String>)> {
    let mut vars: Vec<EnvVarSpec> = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    for (i, item) in list.into_iter().flatten().enumerate() {
        let ictx = format!("{ctx}: envVars[{i}]");
        match parse_env_var(item, &ictx, warnings)? {
            EnvItem::Var(v) => {
                if let Some(pos) = vars.iter().position(|e| e.key() == v.key()) {
                    warnings
                        .push(format!("{ctx}: env var '{}' is defined more than once (the last one wins)", v.key()));
                    vars.remove(pos);
                }
                vars.push(v);
            }
            EnvItem::Group(g) => {
                if !allow_groups {
                    return Err(Error::invalid(format!("{ictx}: fromGroup is only supported in services")));
                }
                if !groups.contains(&g) {
                    groups.push(g);
                }
            }
        }
    }
    Ok((vars, groups))
}

fn parse_env_var(v: &Value, ctx: &str, warnings: &mut Vec<String>) -> Result<EnvItem> {
    let map = as_mapping(v, ctx)?;
    let mut e = Entry::new(map, ctx);
    if e.has("fromGroup") {
        let group = e.required_string("fromGroup")?;
        e.finish(warnings);
        return Ok(EnvItem::Group(group));
    }
    let key = e.required_string("key")?;
    let ctx = format!("{ctx} ({key})");
    let has_value = map.contains_key("value");
    let generate = e.bool("generateValue")?;
    let sync = e.bool("sync")?;
    let from_db = e.mapping("fromDatabase")?;
    let from_svc = e.mapping("fromService")?;
    let sources = [has_value, generate == Some(true), from_db.is_some(), from_svc.is_some()];
    if sources.iter().filter(|b| **b).count() > 1 {
        return Err(Error::invalid(format!(
            "{ctx}: set only one of value, generateValue, fromDatabase or fromService"
        )));
    }

    let spec = if has_value {
        let value = match e.get("value") {
            None => String::new(),
            Some(v) => scalar_string(v)
                .ok_or_else(|| Error::invalid(format!("{ctx}: 'value' must be a string")))?
                .unwrap_or_default(),
        };
        EnvVarSpec::Value { key, value }
    } else if generate == Some(true) {
        EnvVarSpec::Generate { key }
    } else if let Some(db) = from_db {
        let mut r = Entry::new(db, &format!("{ctx}: fromDatabase"));
        let name = r.required_string("name")?;
        let property = r.required_string("property")?;
        r.finish(warnings);
        EnvVarSpec::Reference { key, target: RefTarget::Database, name, property }
    } else if let Some(svc) = from_svc {
        let mut r = Entry::new(svc, &format!("{ctx}: fromService"));
        let name = r.required_string("name")?;
        let ty = r.string("type")?;
        let property = r.string("property")?.filter(|p| !p.trim().is_empty());
        let env_var_key = r.string("envVarKey")?.filter(|p| !p.trim().is_empty());
        let rctx = r.ctx().to_string();
        r.finish(warnings);
        let target = match ty.as_deref().map(|t| t.trim().to_ascii_lowercase()) {
            None => RefTarget::Any,
            Some(t) if matches!(t.as_str(), "redis" | "keyvalue" | "key_value") => RefTarget::KeyValue,
            Some(t) => match ServiceType::from_str(&t) {
                Ok(_) => RefTarget::Service,
                Err(_) => return Err(Error::invalid(format!("{rctx}: unknown type '{t}'"))),
            },
        };
        match (property, env_var_key) {
            (Some(_), Some(_)) => {
                return Err(Error::invalid(format!("{rctx}: set either property or envVarKey, not both")));
            }
            (Some(property), None) => EnvVarSpec::Reference { key, target, name, property: property.trim().into() },
            (None, Some(env_var_key)) => {
                if target.is_datastore() {
                    return Err(Error::invalid(format!("{rctx}: envVarKey is only supported for services")));
                }
                EnvVarSpec::CopyFrom { key, service: name, env_var_key: env_var_key.trim().into() }
            }
            (None, None) => return Err(Error::invalid(format!("{rctx}: 'property' or 'envVarKey' is required"))),
        }
    } else if sync == Some(false) {
        EnvVarSpec::Unsynced { key }
    } else {
        return Err(Error::invalid(format!(
            "{ctx}: needs a value, generateValue: true, fromDatabase, fromService or sync: false"
        )));
    };
    e.finish(warnings);
    Ok(EnvItem::Var(spec))
}

// ---------------------------------------------------------------------------
// entries

fn parse_env_group(v: &Value, idx: usize, warnings: &mut Vec<String>) -> Result<EnvGroupSpec> {
    let map = as_mapping(v, &format!("envVarGroups[{idx}]"))?;
    let mut e = Entry::new(map, &format!("envVarGroups[{idx}]"));
    let name = e.required_string("name")?;
    e.ctx = format!("env group '{name}'");
    let list = e.seq("envVars")?;
    let (env_vars, _) = parse_env_vars(list, e.ctx(), false, warnings)?;
    e.finish(warnings);
    Ok(EnvGroupSpec { name, env_vars })
}

fn parse_database(v: &Value, idx: usize, warnings: &mut Vec<String>) -> Result<DatastoreSpec> {
    let map = as_mapping(v, &format!("databases[{idx}]"))?;
    let mut e = Entry::new(map, &format!("databases[{idx}]"));
    let name = e.required_string("name")?;
    e.ctx = format!("database '{name}'");
    let version = e.string("postgresMajorVersion")?.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let database = e.string("databaseName")?.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let user = e.string("user")?.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let limits = e.limits(Some(&plans::POSTGRES), warnings)?;
    e.finish(warnings);
    Ok(DatastoreSpec { name, kind: DatastoreKind::Postgres, version, database, user, limits })
}

enum Parsed {
    Service(Box<ServiceSpec>),
    Datastore(DatastoreSpec),
}

fn opt_trim(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Strip a leading `./` and trailing `/` for path comparisons.
fn clean_rel(p: &str) -> &str {
    let p = p.trim();
    let p = p.strip_prefix("./").unwrap_or(p);
    let p = p.trim_end_matches('/');
    if p.is_empty() { "." } else { p }
}

fn parse_service(v: &Value, idx: usize, warnings: &mut Vec<String>) -> Result<Parsed> {
    let map = as_mapping(v, &format!("services[{idx}]"))?;
    let mut e = Entry::new(map, &format!("services[{idx}]"));
    let name = e.required_string("name")?;
    e.ctx = format!("service '{name}'");
    let ctx = e.ctx.clone();
    let ty_raw = e.required_string("type")?;
    let ty_lower = ty_raw.to_ascii_lowercase();

    if matches!(ty_lower.as_str(), "redis" | "keyvalue" | "key_value") {
        e.ctx = format!("key value '{name}'");
        let limits = e.limits(Some(&plans::KEY_VALUE), warnings)?;
        e.finish(warnings);
        return Ok(Parsed::Datastore(DatastoreSpec {
            name,
            kind: DatastoreKind::Redis,
            version: None,
            database: None,
            user: None,
            limits,
        }));
    }
    let mut service_type = ServiceType::from_str(&ty_lower).map_err(|_| {
        Error::invalid(format!(
            "{ctx}: unsupported service type '{ty_raw}' (expected web, pserv, worker, cron, static, redis or keyvalue)"
        ))
    })?;

    let mut spec = ServiceSpec::new(name.clone(), service_type);

    // runtime (legacy key: env)
    let runtime_raw = opt_trim(e.string("runtime")?);
    let env_raw = opt_trim(e.string("env")?);
    if let (Some(r), Some(l)) = (&runtime_raw, &env_raw)
        && !r.eq_ignore_ascii_case(l)
    {
        warnings.push(format!("{ctx}: both 'runtime' ({r}) and legacy 'env' ({l}) are set; using runtime"));
    }
    let runtime_raw = runtime_raw.or(env_raw);
    let mut unsupported_runtime = None;
    if let Some(raw) = &runtime_raw {
        match Runtime::from_str(raw) {
            Ok(r) => spec.runtime = Some(r),
            // Reported below, once we know whether an image is set.
            Err(_) => unsupported_runtime = Some(raw.clone()),
        }
    }
    if spec.runtime == Some(Runtime::Static) && service_type == ServiceType::WebService {
        service_type = ServiceType::StaticSite;
        spec.service_type = service_type;
    }
    if service_type == ServiceType::StaticSite && spec.runtime.is_none() {
        spec.runtime = Some(Runtime::Static);
    }

    spec.repo_url = opt_trim(e.string("repo")?);
    spec.branch = opt_trim(e.string("branch")?);

    // image: {url, creds} or a plain string
    if let Some(img) = e.get("image") {
        match img {
            Value::Mapping(m) => {
                let mut ie = Entry::new(m, &format!("{ctx}: image"));
                spec.image = opt_trim(ie.string("url")?);
                if spec.image.is_none() {
                    return Err(Error::invalid(format!("{ctx}: image.url is required")));
                }
                if ie.has("creds") {
                    ie.skip("creds");
                    warnings.push(format!(
                        "{ctx}: registry credentials are not supported; the image must be pullable anonymously"
                    ));
                }
                ie.finish(warnings);
            }
            other => {
                spec.image =
                    opt_trim(scalar_string(other).ok_or_else(|| Error::invalid(format!("{ctx}: invalid image")))?);
            }
        }
    }

    match (&spec.image, &unsupported_runtime) {
        (Some(image), Some(raw)) => {
            warnings.push(format!("{ctx}: runtime '{raw}' is ignored: the service runs the image '{image}'"))
        }
        (None, Some(raw)) => warnings
            .push(format!("{ctx}: runtime '{raw}' is not supported natively; using auto-detection (add a Dockerfile)")),
        (Some(image), None) if runtime_raw.is_some() => {
            // An explicit, supported runtime that the image makes moot.
            if let Some(r) = spec.runtime.filter(|r| *r != Runtime::Image) {
                warnings.push(format!("{ctx}: runtime '{r}' is ignored: the service runs the image '{image}'"));
                spec.runtime = None;
            }
        }
        (_, None) => {}
    }

    // build settings
    let root_dir = opt_trim(e.string("rootDir")?);
    let docker_context = opt_trim(e.string("dockerContext")?);
    let mut dockerfile = opt_trim(e.string("dockerfilePath")?);
    spec.root_dir = root_dir.clone();
    if let Some(ctx_dir) = docker_context.filter(|c| clean_rel(c) != ".") {
        if root_dir.is_some() {
            warnings
                .push(format!("{ctx}: dockerContext is ignored when rootDir is set (rootDir is the build context)"));
        } else {
            // Ferry's build context is root_dir and the Dockerfile path is
            // relative to it; Render's dockerfilePath is relative to the repo.
            let prefix = clean_rel(&ctx_dir).to_string();
            if let Some(df) = &dockerfile {
                let rel = clean_rel(df);
                match rel.strip_prefix(&format!("{prefix}/")) {
                    Some(inner) => dockerfile = Some(format!("./{inner}")),
                    None => warnings.push(format!(
                        "{ctx}: dockerfilePath '{df}' is outside dockerContext '{ctx_dir}'; it is used relative to the context"
                    )),
                }
            }
            spec.root_dir = Some(ctx_dir);
        }
    }
    spec.dockerfile_path = dockerfile;
    spec.build_command = opt_trim(e.string("buildCommand")?);
    let start = opt_trim(e.string("startCommand")?);
    let docker_cmd = opt_trim(e.string("dockerCommand")?);
    if let (Some(s), Some(d)) = (&start, &docker_cmd)
        && s != d
    {
        warnings.push(format!("{ctx}: both startCommand and dockerCommand are set; using startCommand"));
    }
    spec.start_command = start.or(docker_cmd);
    spec.publish_dir = opt_trim(e.string("staticPublishPath")?);

    // runtime settings
    spec.port = e.port("port")?;
    spec.health_check_path = opt_trim(e.string("healthCheckPath")?);
    spec.instances = e.number::<u32>("numInstances")?;
    if service_type == ServiceType::CronJob && spec.instances.take().is_some() {
        warnings.push(format!(
            "{ctx}: numInstances is ignored: cron jobs have no long-running instances (each run starts one container)"
        ));
    }
    spec.auto_deploy = e.bool("autoDeploy")?;
    if let Some(trigger) = opt_trim(e.string("autoDeployTrigger")?) {
        let on = match trigger.to_ascii_lowercase().as_str() {
            // `off` may arrive as a YAML 1.1 boolean from other tooling.
            "off" | "false" | "no" => false,
            "commit" | "true" | "yes" => true,
            "checkspass" => {
                warnings
                    .push(format!("{ctx}: autoDeployTrigger 'checksPass' is not supported; deploying on every commit"));
                true
            }
            _ => return Err(Error::invalid(format!("{ctx}: invalid autoDeployTrigger '{trigger}'"))),
        };
        if spec.auto_deploy.is_some_and(|a| a != on) {
            warnings.push(format!("{ctx}: autoDeploy and autoDeployTrigger disagree; using autoDeployTrigger"));
        }
        spec.auto_deploy = Some(on);
    }
    spec.schedule = opt_trim(e.string("schedule")?);
    if let Some(disk) = e.mapping("disk")? {
        let mut d = Entry::new(disk, &format!("{ctx}: disk"));
        spec.disk_mount_path = Some(d.required_string("mountPath")?);
        d.skip("name");
        d.skip("sizeGB");
        d.finish(warnings);
    }
    spec.domains = e
        .string_list("domains")?
        .map(|v| v.into_iter().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect());
    // Static sites have no plans on Render; their containers still take
    // memoryLimit / cpuLimit.
    let plan_table = (service_type != ServiceType::StaticSite).then_some(&plans::SERVICES);
    spec.limits = e.limits(plan_table, warnings)?;

    let list = e.seq("envVars")?;
    let (vars, groups) = parse_env_vars(list, &ctx, true, warnings)?;
    spec.env_vars = vars;
    spec.env_groups = groups;
    e.finish(warnings);
    Ok(Parsed::Service(Box::new(spec)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_and_empty() {
        assert!(parse("").unwrap().warnings.iter().any(|w| w.contains("empty")));
        assert!(parse("- a").is_err());
        assert!(parse("services: [").is_err());
        let bp = parse("services:\n  - type: worker\n    name: bg\n").unwrap();
        assert_eq!(bp.services[0].service_type, ServiceType::BackgroundWorker);
        assert!(bp.warnings.is_empty(), "{:?}", bp.warnings);
    }

    #[test]
    fn errors_name_the_entry() {
        let err = parse("services:\n  - type: nope\n    name: x\n").unwrap_err().to_string();
        assert!(err.contains("service 'x'") && err.contains("nope"), "{err}");
        let err = parse("services:\n  - type: web\n").unwrap_err().to_string();
        assert!(err.contains("services[0]") && err.contains("name"), "{err}");
        let err = parse("services:\n  - type: web\n    name: a\n    envVars:\n      - key: A\n").unwrap_err();
        assert!(err.to_string().contains("(A)"), "{err}");
        let err = parse("envVarGroups:\n  - name: g\n    envVars:\n      - fromGroup: other\n").unwrap_err();
        assert!(err.to_string().contains("only supported in services"));
        let err = parse(
            "services:\n  - type: web\n    name: a\n    envVars:\n      - {key: A, value: x, generateValue: true}\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("only one"));
    }

    #[test]
    fn static_and_legacy_env() {
        let bp = parse(
            "services:\n  - type: web\n    name: site\n    env: static\n    staticPublishPath: ./dist\n  - type: static\n    name: s2\n",
        )
        .unwrap();
        assert_eq!(bp.services[0].service_type, ServiceType::StaticSite);
        assert_eq!(bp.services[0].runtime, Some(Runtime::Static));
        assert_eq!(bp.services[0].publish_dir.as_deref(), Some("./dist"));
        assert_eq!(bp.services[1].runtime, Some(Runtime::Static));
    }

    #[test]
    fn docker_context_and_merge_keys() {
        let bp = parse(
            "x-common: &common\n  plan: starter\n  region: oregon\nservices:\n  - <<: *common\n    type: web\n    name: api\n    runtime: docker\n    dockerContext: ./api\n    dockerfilePath: ./api/Dockerfile.prod\n    autoDeployTrigger: \"off\"\n    image: nginx:1\n",
        )
        .unwrap();
        let s = &bp.services[0];
        assert_eq!(s.root_dir.as_deref(), Some("./api"));
        assert_eq!(s.dockerfile_path.as_deref(), Some("./Dockerfile.prod"));
        assert_eq!(s.auto_deploy, Some(false));
        assert_eq!(s.image.as_deref(), Some("nginx:1"));
        assert_eq!(s.limits, LimitsSpec { memory_mb: Some(512), cpus: Some(0.5) });
        for key in ["x-common", "region"] {
            assert!(bp.warnings.iter().any(|w| w.contains(&format!("'{key}'"))), "{key}: {:?}", bp.warnings);
        }
        assert!(!bp.warnings.iter().any(|w| w.contains("plan")), "{:?}", bp.warnings);
    }

    #[test]
    fn plans_and_limit_keys() {
        let bp = parse(
            r#"
services:
  - {type: web, name: none}
  - {type: web, name: std, plan: standard}
  - {type: worker, name: big, plan: Pro_Plus}
  - {type: cron, name: tick, schedule: "* * * * *", plan: pro, memoryLimit: 1G}
  - {type: pserv, name: own, plan: starter, memoryLimit: 768, cpuLimit: 250m}
  - {type: web, name: plain, memoryLimit: 1.5G, cpuLimit: 2}
  - {type: web, name: reset, plan: pro, memoryLimit: 0, cpuLimit: 0}
  - {type: static, name: site, plan: starter}
  - {type: web, name: site2, runtime: static, plan: free, memoryLimit: 64M}
  - {type: web, name: odd, plan: gigantic}
  - {type: redis, name: kv, plan: pro}
  - {type: keyvalue, name: kv2, plan: pro plus, cpuLimit: 0.5, maxmemoryPolicy: noeviction}
  - {type: keyvalue, name: kv3, plan: accelerated-16gb}
databases:
  - {name: db, plan: basic-1gb}
  - {name: legacy, plan: starter, cpuLimit: 1}
  - {name: huge, plan: accelerated-64gb, memoryLimit: 8G}
  - {name: plain-db}
"#,
        )
        .unwrap();
        let svc = |n: &str| bp.services.iter().find(|s| s.name == n).unwrap().limits;
        let ds = |n: &str| bp.datastores.iter().find(|d| d.name == n).unwrap().limits;
        let l = |m: Option<u32>, c: Option<f64>| LimitsSpec { memory_mb: m, cpus: c };
        assert_eq!(svc("none"), l(None, None));
        assert_eq!(svc("std"), l(Some(2048), Some(1.0)));
        assert_eq!(svc("big"), l(Some(8192), Some(4.0)));
        // the extension keys override the plan, key by key
        assert_eq!(svc("tick"), l(Some(1024), Some(2.0)));
        assert_eq!(svc("own"), l(Some(768), Some(0.25)));
        assert_eq!(svc("plain"), l(Some(1536), Some(2.0)));
        assert_eq!(svc("reset"), l(Some(0), Some(0.0)));
        // static sites: no plans, but the limit keys apply
        assert_eq!(svc("site"), l(None, None));
        assert_eq!(svc("site2"), l(Some(64), None));
        assert_eq!(svc("odd"), l(None, None));
        // datastores: memory from the plan, CPU only from cpuLimit
        assert_eq!(ds("kv"), l(Some(5120), None));
        assert_eq!(ds("kv2"), l(Some(10240), Some(0.5)));
        assert_eq!(ds("kv3"), l(None, None));
        assert_eq!(ds("db"), l(Some(1024), None));
        assert_eq!(ds("legacy"), l(Some(256), Some(1.0)));
        assert_eq!(ds("huge"), l(Some(8192), None));
        assert_eq!(ds("plain-db"), l(None, None));

        let unknown: Vec<&String> = bp.warnings.iter().filter(|w| w.contains("unknown plan")).collect();
        assert_eq!(unknown.len(), 2, "{:?}", bp.warnings);
        assert!(unknown[0].starts_with("service 'odd': ignoring unknown plan 'gigantic'"), "{unknown:?}");
        assert!(unknown[0].contains("pro ultra") && unknown[0].contains("memoryLimit"), "{unknown:?}");
        assert!(unknown[1].starts_with("key value 'kv3': ignoring unknown plan 'accelerated-16gb'"), "{unknown:?}");
        // no "unsupported key" warnings for plan, memoryLimit or cpuLimit
        assert_eq!(bp.warnings.len(), 3, "{:?}", bp.warnings);
        assert!(bp.warnings.iter().any(|w| w.contains("'maxmemoryPolicy'")), "{:?}", bp.warnings);
    }

    #[test]
    fn malformed_limits_are_errors_naming_the_entry() {
        for (entry, want) in [
            (
                "services:\n  - {type: web, name: a, memoryLimit: lots}\n",
                "service 'a': 'memoryLimit': invalid memory size 'lots'",
            ),
            ("services:\n  - {type: web, name: a, cpuLimit: -1}\n", "service 'a': 'cpuLimit': invalid CPU amount '-1'"),
            ("services:\n  - {type: redis, name: kv, memoryLimit: 1X}\n", "key value 'kv': 'memoryLimit'"),
            ("databases:\n  - {name: db, cpuLimit: many}\n", "database 'db': 'cpuLimit'"),
            // Rounds to 0 (the server default): refused, not silently reset.
            (
                "services:\n  - {type: web, name: a, cpuLimit: 4m}\n",
                "service 'a': 'cpuLimit': CPU limit must be between 0.01 and 512 CPUs (got 4m)",
            ),
            ("databases:\n  - {name: db, cpuLimit: 0.004}\n", "database 'db': 'cpuLimit': CPU limit must be between"),
            ("services:\n  - {type: web, name: a, memoryLimit: [1]}\n", "service 'a': 'memoryLimit' must be a string"),
        ] {
            let err = parse(entry).unwrap_err().to_string();
            assert!(err.contains(want), "{entry}: {err}");
        }
    }

    #[test]
    fn env_var_forms() {
        let bp = parse(
            r#"
services:
  - type: web
    name: api
    envVars:
      - key: A
        value: 1
      - key: B
        generateValue: true
      - key: C
        sync: false
      - key: D
        fromDatabase: {name: db, property: connectionString}
      - key: E
        fromService: {type: redis, name: cache, property: connectionString}
      - key: F
        fromService: {type: pserv, name: other, envVarKey: TOKEN}
      - key: G
        fromService: {name: other, property: hostport}
      - fromGroup: shared
      - key: A
        value: "2"
        previewValue: x
"#,
        )
        .unwrap();
        let s = &bp.services[0];
        assert_eq!(s.env_groups, vec!["shared"]);
        assert_eq!(s.env_vars.len(), 7);
        assert!(s.env_vars.contains(&EnvVarSpec::Value { key: "A".into(), value: "2".into() }));
        assert!(s.env_vars.contains(&EnvVarSpec::Generate { key: "B".into() }));
        assert!(s.env_vars.contains(&EnvVarSpec::Unsynced { key: "C".into() }));
        assert!(s.env_vars.contains(&EnvVarSpec::Reference {
            key: "E".into(),
            target: RefTarget::KeyValue,
            name: "cache".into(),
            property: "connectionString".into()
        }));
        assert!(s.env_vars.contains(&EnvVarSpec::CopyFrom {
            key: "F".into(),
            service: "other".into(),
            env_var_key: "TOKEN".into()
        }));
        assert!(s.env_vars.contains(&EnvVarSpec::Reference {
            key: "G".into(),
            target: RefTarget::Any,
            name: "other".into(),
            property: "hostport".into()
        }));
        assert!(bp.warnings.iter().any(|w| w.contains("more than once")));
        assert!(bp.warnings.iter().any(|w| w.contains("previewValue")));
    }

    #[test]
    fn scalars_are_kept_exactly_as_written() {
        let cases = [
            ("3.10", "3.10"),
            ("0x1F", "0x1F"),
            ("0o17", "0o17"),
            ("1e3", "1e3"),
            ("True", "True"),
            ("+5", "+5"),
            (".5", ".5"),
            ("0.30", "0.30"),
            ("0123", "0123"),
            ("18446744073709551616", "18446744073709551616"),
            ("-9223372036854775809", "-9223372036854775809"),
            ("123456789012345678901234567890123456789012345", "123456789012345678901234567890123456789012345"),
            ("'3.10'", "3.10"),
            ("\"yes\"", "yes"),
            ("!!str 3.10", "3.10"),
        ];
        let mut yaml = String::from("services:\n  - type: worker\n    name: w\n    branch: 1.10\n    envVars:\n");
        for (i, (lit, _)) in cases.iter().enumerate() {
            yaml.push_str(&format!("      - key: K{i}\n        value: {lit}\n"));
        }
        let bp = parse(&yaml).unwrap();
        let s = &bp.services[0];
        assert_eq!(s.branch.as_deref(), Some("1.10"));
        for (i, (lit, want)) in cases.iter().enumerate() {
            let got = s.env_vars.iter().find_map(|v| match v {
                EnvVarSpec::Value { key, value } if *key == format!("K{i}") => Some(value.as_str()),
                _ => None,
            });
            assert_eq!(got, Some(*want), "{lit}");
        }
        assert!(bp.warnings.is_empty(), "{:?}", bp.warnings);
    }

    #[test]
    fn typed_fields_parse_their_literal_text() {
        let bp = parse(
            "x-v: &v 3.10\nservices:\n  - type: web\n    name: a\n    numInstances: 2\n    port: 8080\n    autoDeploy: True\n    envVars:\n      - {key: A, value: *v}\n      - {key: B, value: ~}\n      - {key: C, generateValue: yes}\n",
        )
        .unwrap();
        let s = &bp.services[0];
        assert_eq!((s.instances, s.port, s.auto_deploy), (Some(2), Some(8080), Some(true)));
        assert!(s.env_vars.contains(&EnvVarSpec::Value { key: "A".into(), value: "3.10".into() }));
        assert!(s.env_vars.contains(&EnvVarSpec::Value { key: "B".into(), value: String::new() }));
        assert!(s.env_vars.contains(&EnvVarSpec::Generate { key: "C".into() }));
        for (port, ok) in [("70000", false), ("0", false), ("-1", false), ("65535", true), ("\"80\"", true)] {
            let r = parse(&format!("services:\n  - {{type: web, name: a, port: {port}}}\n"));
            match r {
                Ok(bp) => assert!(ok, "{port}: {:?}", bp.services[0].port),
                Err(e) => {
                    assert!(!ok, "{port}: {e}");
                    assert!(e.to_string().contains("between 1 and 65535"), "{e}");
                }
            }
        }
        let err = parse("services:\n  - {type: web, name: a}\n  - {type: web, name: b, name: c}\n").unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn projects_are_flattened() {
        let bp = parse(
            "projects:\n  - name: p\n    environments:\n      - name: prod\n        services:\n          - {type: worker, name: w}\n        databases:\n          - {name: db}\n",
        )
        .unwrap();
        assert_eq!(bp.services.len(), 1);
        assert_eq!(bp.datastores.len(), 1);
        assert!(bp.warnings.iter().any(|w| w.contains("projects")));
    }
}
