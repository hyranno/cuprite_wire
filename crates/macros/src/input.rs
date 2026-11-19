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
                        options.fields = string_map(content)?;
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
                if let Some(recorded) = common_key(key, content, &mut options.common)? {
                    return Ok(recorded.map(|k| vec![(k, span)]).unwrap_or_default());
                }
                match model_key(key, content, &mut options.model)? {
                    Some(field)
                        if field != BindingField::Pins
                            && field != BindingField::Unused
                            && field != BindingField::Suffix =>
                    {
                        Ok(vec![(OptionKey::Model { element: 0, field }, span)])
                    }
                    _ => Err(unknown_key(key, SPICE_KEYS)),
                }
            },
            &mut spans,
        )?;
        for (field, what) in [
            (BindingField::Library, "library: \"path\""),
            (BindingField::Name, "name: \"NAME\""),
        ] {
            if !spans.has(OptionKey::Model { element: 0, field }) {
                return Err(syn::Error::new(
                    header.name.span(),
                    format!("`{what}` is required"),
                ));
            }
        }
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
        for (field, what) in [
            (BindingField::Library, "library"),
            (BindingField::Name, "name"),
        ] {
            if !spans.has(OptionKey::Model { element, field }) {
                return Err(syn::Error::new(
                    block,
                    format!("`{what}` is required in `sim`"),
                ));
            }
        }
        spans.models.push(block);
        bindings.push(binding);
        Ok(())
    };
    if input.peek(syn::token::Bracket) {
        let list;
        bracketed!(list in input);
        while !list.is_empty() {
            one(&list)?;
            if list.is_empty() {
                break;
            }
            list.parse::<Token![,]>()?;
        }
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
    while !content.is_empty() {
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
        if content.is_empty() {
            break;
        }
        content.parse::<Token![,]>()?;
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
            let mut out = Vec::new();
            while !groups.is_empty() {
                out.push(string_list(&groups)?);
                if groups.is_empty() {
                    break;
                }
                groups.parse::<Token![,]>()?;
            }
            options.internal_connections = out;
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

/// `{ "a" => "b", ... }`
fn string_map(input: ParseStream) -> syn::Result<Vec<(String, String)>> {
    let content;
    braced!(content in input);
    let entries = Punctuated::<Entry, Token![,]>::parse_terminated(&content)?;
    Ok(entries
        .into_iter()
        .map(|e| (e.key.value(), e.value.value()))
        .collect())
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
