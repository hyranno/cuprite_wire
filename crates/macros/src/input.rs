//! Parsing the input of `kicad_part!` and `spice_part!`.

use cuprite_wire_kicad_lib::{
    binding::{BindingField, DialectOption, ModelBinding},
    part::{CommonOptions, FootprintOption, OptionKey, PartOptions, SpiceOption, SpicePartOptions},
    sim::{ModelMode, Value},
    spice_lib::Dialect,
};
use proc_macro2::Span;
use syn::{
    Attribute, Ident, LitStr, Token, Visibility, braced, bracketed,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

/// `#[attrs] vis struct Name`
pub struct Header {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub name: Ident,
}

impl Parse for Header {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis: Visibility = input.parse()?;
        input.parse::<Token![struct]>()?;
        let name: Ident = input.parse()?;
        Ok(Header { attrs, vis, name })
    }
}

/// Where each option is written, to point errors at it.
#[derive(Default)]
pub struct Spans {
    options: Vec<(OptionKey, Span)>,
    /// The blocks of the `sim` models.
    models: Vec<Span>,
}

impl Spans {
    /// The span of the option `key`; for an option of a model that is not written, the model's
    /// block; else the span of `fallback`, else `default`.
    pub fn of(&self, key: OptionKey, fallback: OptionKey, default: Span) -> Span {
        let find = |key| {
            self.options
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, span)| *span)
        };
        let model = match key {
            OptionKey::Model { element, .. } => self.models.get(element).copied(),
            _ => None,
        };
        find(key)
            .or(model)
            .or_else(|| find(fallback))
            .unwrap_or(default)
    }

    fn has(&self, key: OptionKey) -> bool {
        self.options.iter().any(|(k, _)| *k == key)
    }

    /// An error at `span` if the `library` or `name` of the model `element` is not written;
    /// `message` makes it from the key.
    fn require_library_and_name(
        &self,
        element: usize,
        span: Span,
        message: impl Fn(&str) -> String,
    ) -> syn::Result<()> {
        for (field, key) in [
            (BindingField::Library, "library"),
            (BindingField::Name, "name"),
        ] {
            if !self.has(OptionKey::Model { element, field }) {
                return Err(syn::Error::new(span, message(key)));
            }
        }
        Ok(())
    }
}

/// `#[attrs] vis struct Name { key: value, ... }` of `kicad_part!`.
pub struct PartInput {
    pub header: Header,
    pub options: PartOptions,
    pub symbol_dir: Option<LitStr>,
    pub footprint_dir: Option<LitStr>,
    pub sim_library_dir: Option<LitStr>,
    pub spans: Spans,
}

/// `#[attrs] vis struct Name { key: value, ... }` of `spice_part!`.
pub struct SpicePartInput {
    pub header: Header,
    pub options: SpicePartOptions,
    pub spans: Spans,
}

const KICAD_KEYS: &str = "symbol, footprint, value, ref_base, pins, internal_connections, extra_pads, \
                          spice, sim, sim_model, fields, symbol_dir, footprint_dir, sim_library_dir";
const SPICE_KEYS: &str =
    "library, name, value, ref_base, pins, internal_connections, params, dialect, sim_model";
const SIM_KEYS: &str = "library, name, pins, unused, params, dialect, suffix";

impl Parse for PartInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let header: Header = input.parse()?;
        let mut options = PartOptions {
            common: CommonOptions {
                type_name: header.name.to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let (mut symbol_dir, mut footprint_dir, mut sim_library_dir) = (None, None, None);
        let mut spans = Spans::default();
        let mut sim_spans = Spans::default();
        let mut has_symbol = false;
        let content;
        braced!(content in input);
        parse_keys(
            &content,
            |key, content, span| {
                if let Some(recorded) = common_key(key, content, &mut options.common)? {
                    return Ok(recorded.map(|k| vec![(k, span)]).unwrap_or_default());
                }
                let recorded = match key.to_string().as_str() {
                    "symbol" => {
                        options.symbol = content.parse::<LitStr>()?.value();
                        has_symbol = true;
                        Some(OptionKey::Symbol)
                    }
                    "footprint" => {
                        options.footprint = if content.peek(LitStr) {
                            FootprintOption::Given(content.parse::<LitStr>()?.value())
                        } else {
                            keyword(content, &["none"])?;
                            FootprintOption::None
                        };
                        Some(OptionKey::Footprint)
                    }
                    "fields" => {
                        options.fields = fields(content)?;
                        None
                    }
                    "extra_pads" => {
                        options.extra_pads = string_list(content)?;
                        Some(OptionKey::ExtraPads)
                    }
                    "spice" => {
                        options.spice =
                            match keyword(content, &["auto", "none", "excluded"])?.as_str() {
                                "auto" => SpiceOption::Auto,
                                "none" => SpiceOption::None,
                                _ => SpiceOption::Excluded,
                            };
                        Some(OptionKey::Spice)
                    }
                    "sim" => {
                        options.sim = sim_block(content, &mut sim_spans)?;
                        return Ok(Vec::new());
                    }
                    "symbol_dir" => {
                        symbol_dir = Some(content.parse()?);
                        None
                    }
                    "footprint_dir" => {
                        footprint_dir = Some(content.parse()?);
                        None
                    }
                    "sim_library_dir" => {
                        sim_library_dir = Some(content.parse()?);
                        None
                    }
                    _ => return Err(unknown_key(key, KICAD_KEYS)),
                };
                Ok(recorded.map(|k| vec![(k, span)]).unwrap_or_default())
            },
            &mut spans,
        )?;
        if !has_symbol {
            return Err(syn::Error::new(
                header.name.span(),
                "`symbol: \"Library:Name\"` is required",
            ));
        }
        spans.options.extend(sim_spans.options);
        spans.models = sim_spans.models;
        end(input)?;
        Ok(PartInput {
            header,
            options,
            symbol_dir,
            footprint_dir,
            sim_library_dir,
            spans,
        })
    }
}

impl Parse for SpicePartInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let header: Header = input.parse()?;
        let mut options = SpicePartOptions {
            common: CommonOptions {
                type_name: header.name.to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut spans = Spans::default();
        let content;
        braced!(content in input);
        parse_keys(
            &content,
            |key, content, span| {
                // Before the value is parsed: `sim` keys such as `suffix` are not options here.
                if !SPICE_KEYS.split(", ").any(|k| key == k) {
                    return Err(unknown_key(key, SPICE_KEYS));
                }
                if let Some(recorded) = common_key(key, content, &mut options.common)? {
                    return Ok(recorded.map(|k| vec![(k, span)]).unwrap_or_default());
                }
                match model_key(key, content, &mut options.model)? {
                    Some(field) => Ok(vec![(OptionKey::Model { element: 0, field }, span)]),
                    None => Err(unknown_key(key, SPICE_KEYS)),
                }
            },
            &mut spans,
        )?;
        spans.require_library_and_name(0, header.name.span(), |key| match key {
            "library" => "`library: \"path\"` is required".into(),
            _ => "`name: \"NAME\"` is required".into(),
        })?;
        end(input)?;
        Ok(SpicePartInput {
            header,
            options,
            spans,
        })
    }
}

/// `sim: { ... }` or `sim: [{ ... }, ...]`.
fn sim_block(input: ParseStream, spans: &mut Spans) -> syn::Result<Vec<ModelBinding>> {
    let mut bindings = Vec::new();
    let mut one = |input: ParseStream| -> syn::Result<()> {
        let element = bindings.len();
        let mut binding = ModelBinding::default();
        let block = input
            .cursor()
            .token_tree()
            .map_or_else(|| input.span(), |(tree, _)| tree.span());
        let content;
        braced!(content in input);
        parse_keys(
            &content,
            |key, content, span| match model_key(key, content, &mut binding)? {
                Some(field) => Ok(vec![(OptionKey::Model { element, field }, span)]),
                None => Err(unknown_key(key, SIM_KEYS)),
            },
            spans,
        )?;
        spans.require_library_and_name(element, block, |key| {
            format!("`{key}` is required in `sim`")
        })?;
        spans.models.push(block);
        bindings.push(binding);
        Ok(())
    };
    if input.peek(syn::token::Bracket) {
        let list;
        bracketed!(list in input);
        comma_separated(&list, one)?;
    } else {
        one(input)?;
    }
    Ok(bindings)
}

/// Parses `key: value` pairs separated by commas. `handle` parses the value and returns the
/// options it sets with their spans.
fn parse_keys(
    content: ParseStream,
    mut handle: impl FnMut(&Ident, ParseStream, Span) -> syn::Result<Vec<(OptionKey, Span)>>,
    spans: &mut Spans,
) -> syn::Result<()> {
    let mut seen: Vec<String> = Vec::new();
    comma_separated(content, |content| {
        let key: Ident = content.parse()?;
        let key_name = key.to_string();
        if seen.contains(&key_name) {
            return Err(syn::Error::new(
                key.span(),
                format!("`{key_name}` is given twice"),
            ));
        }
        seen.push(key_name);
        content.parse::<Token![:]>()?;
        // The whole value (a group's span covers its brackets and contents).
        let span = content
            .cursor()
            .token_tree()
            .map_or_else(|| content.span(), |(tree, _)| tree.span());
        spans.options.extend(handle(&key, content, span)?);
        Ok(())
    })
}

/// Calls `item` for each item of the comma-separated list filling `input`; a trailing comma is
/// allowed.
fn comma_separated(
    input: ParseStream,
    mut item: impl FnMut(ParseStream) -> syn::Result<()>,
) -> syn::Result<()> {
    while !input.is_empty() {
        item(input)?;
        if input.is_empty() {
            break;
        }
        input.parse::<Token![,]>()?;
    }
    Ok(())
}

/// Options shared by both macros. `Ok(None)`: not one of them; `Ok(Some(key))`: parsed, with the
/// key errors point at.
fn common_key(
    key: &Ident,
    content: ParseStream,
    options: &mut CommonOptions,
) -> syn::Result<Option<Option<OptionKey>>> {
    Ok(Some(match key.to_string().as_str() {
        "value" => {
            options.value = Some(if content.peek(LitStr) {
                Value::Fixed(content.parse::<LitStr>()?.value())
            } else {
                keyword(content, &["param"])?;
                Value::Param
            });
            Some(OptionKey::Value)
        }
        "ref_base" => {
            options.ref_base = Some(content.parse::<LitStr>()?.value());
            Some(OptionKey::RefBase)
        }
        "pins" => {
            options.pins = string_map(content)?;
            Some(OptionKey::Pins)
        }
        "internal_connections" => {
            let groups;
            bracketed!(groups in content);
            options.internal_connections =
                Punctuated::<_, Token![,]>::parse_terminated_with(&groups, string_list)?
                    .into_iter()
                    .collect();
            Some(OptionKey::InternalConnections)
        }
        "sim_model" => {
            options.sim_model = match keyword(content, &["include", "inline"])?.as_str() {
                "include" => ModelMode::Include,
                _ => ModelMode::Inline,
            };
            None
        }
        _ => return Ok(None),
    }))
}

/// Options of a model (`spice_part!` and the `sim` block). `Ok(None)`: not one of them.
fn model_key(
    key: &Ident,
    content: ParseStream,
    binding: &mut ModelBinding,
) -> syn::Result<Option<BindingField>> {
    Ok(Some(match key.to_string().as_str() {
        "library" => {
            binding.library = content.parse::<LitStr>()?.value();
            BindingField::Library
        }
        "name" => {
            binding.name = content.parse::<LitStr>()?.value();
            BindingField::Name
        }
        "pins" => {
            binding.pins = string_map(content)?;
            BindingField::Pins
        }
        "unused" => {
            binding.unused = string_list(content)?;
            BindingField::Unused
        }
        "params" => {
            binding.params = string_map(content)?;
            BindingField::Params
        }
        "dialect" => {
            binding.dialect =
                match keyword(content, &["auto", "ngspice", "pspice", "ltspice"])?.as_str() {
                    "ngspice" => DialectOption::Given(Dialect::Ngspice),
                    "pspice" => DialectOption::Given(Dialect::Pspice),
                    "ltspice" => DialectOption::Given(Dialect::Ltspice),
                    _ => DialectOption::Auto,
                };
            BindingField::Dialect
        }
        "suffix" => {
            binding.suffix = content.parse::<LitStr>()?.value();
            BindingField::Suffix
        }
        _ => return Ok(None),
    }))
}

fn unknown_key(key: &Ident, keys: &str) -> syn::Error {
    syn::Error::new(
        key.span(),
        format!("unknown key `{key}`; expected one of: {keys}"),
    )
}

fn end(input: ParseStream) -> syn::Result<()> {
    if input.is_empty() {
        Ok(())
    } else {
        Err(input.error("unexpected tokens after the part definition"))
    }
}

/// One of `allowed` as a bare identifier.
fn keyword(input: ParseStream, allowed: &[&str]) -> syn::Result<String> {
    let ident: Ident = input.parse()?;
    let text = ident.to_string();
    if allowed.contains(&text.as_str()) {
        Ok(text)
    } else {
        let expected: Vec<String> = allowed.iter().map(|a| format!("`{a}`")).collect();
        Err(syn::Error::new(
            ident.span(),
            format!("expected {}", expected.join(" or ")),
        ))
    }
}

/// `["a", "b"]`
fn string_list(input: ParseStream) -> syn::Result<Vec<String>> {
    let content;
    bracketed!(content in input);
    let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
    Ok(items.iter().map(LitStr::value).collect())
}

/// `{ "a" => "b", ... }` with distinct keys.
fn string_map(input: ParseStream) -> syn::Result<Vec<(String, String)>> {
    Ok(pairs(entries(input)?))
}

fn pairs(entries: Vec<Entry>) -> Vec<(String, String)> {
    entries
        .into_iter()
        .map(|e| (e.key.value(), e.value.value()))
        .collect()
}

/// The entries of `{ "a" => "b", ... }`, rejecting a key given twice.
fn entries(input: ParseStream) -> syn::Result<Vec<Entry>> {
    let content;
    braced!(content in input);
    let entries: Vec<Entry> = Punctuated::<Entry, Token![,]>::parse_terminated(&content)?
        .into_iter()
        .collect();
    let mut keys = std::collections::BTreeSet::new();
    for entry in &entries {
        if !keys.insert(entry.key.value()) {
            return Err(syn::Error::new(
                entry.key.span(),
                format!("{:?} is given twice", entry.key.value()),
            ));
        }
    }
    Ok(entries)
}

/// `fields`: the `Sim.*` fields are written from the `sim` block (or are the symbol's), so the
/// user cannot give them.
fn fields(input: ParseStream) -> syn::Result<Vec<(String, String)>> {
    let entries = entries(input)?;
    if let Some(entry) = entries
        .iter()
        .find(|e| e.key.value().to_ascii_lowercase().starts_with("sim."))
    {
        return Err(syn::Error::new(
            entry.key.span(),
            "`Sim.*` fields cannot be given in `fields`; give the model with `sim`",
        ));
    }
    Ok(pairs(entries))
}

struct Entry {
    key: LitStr,
    value: LitStr,
}

impl Parse for Entry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: LitStr = input.parse()?;
        input.parse::<Token![=>]>()?;
        let value: LitStr = input.parse()?;
        if key.value().is_empty() {
            return Err(syn::Error::new(key.span(), "empty key"));
        }
        Ok(Entry { key, value })
    }
}
