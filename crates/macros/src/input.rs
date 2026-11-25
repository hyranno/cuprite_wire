//! Parsing the input of `kicad_part!` and `spice_part!`.

use cuprite_wire_kicad_lib::{
    binding::{BindingField, DialectOption, ModelBinding},
    part::{CommonOptions, FootprintOption, OptionKey, PartOptions, SpiceOption, SpicePartOptions},
    sim::{
        ModelMode, Value,
        ibis::{IbisField, IbisSource},
    },
    spice_lib::Dialect,
};
use proc_macro2::{Ident as Ident2, Span, TokenStream, TokenTree};
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

/// Where something is written, from its first token to its last. A [`Span`] covering several
/// tokens cannot be made on stable Rust (`Span::join` is nightly), so an error over them is
/// given the first and the last.
#[derive(Clone, Copy)]
pub struct Place {
    start: Span,
    end: Span,
}

impl Place {
    fn new(start: Span, end: Span) -> Self {
        Place { start, end }
    }

    /// An error over the place.
    pub fn error(self, message: impl std::fmt::Display) -> syn::Error {
        // `new_spanned` spans the first token to the last.
        let tokens: TokenStream = [self.start, self.end]
            .into_iter()
            .map(|span| TokenTree::from(Ident2::new("x", span)))
            .collect();
        syn::Error::new_spanned(tokens, message)
    }
}

impl From<Span> for Place {
    fn from(span: Span) -> Self {
        Place::new(span, span)
    }
}

/// An option as written: the whole value, and its entries if it is a map or a list.
struct Written {
    key: OptionKey,
    value: Span,
    entries: Vec<Place>,
}

/// Where each option is written, to point errors at it.
#[derive(Default)]
pub struct Spans {
    options: Vec<Written>,
    /// The blocks of the `sim` models.
    models: Vec<Span>,
}

impl Spans {
    /// The place of the `entry`-th entry of the option `key`, else of the whole option; for an
    /// option of a model that is not written, the model's block; else the place of `fallback`,
    /// else `default`.
    pub fn of(
        &self,
        key: OptionKey,
        entry: Option<usize>,
        fallback: OptionKey,
        default: Span,
    ) -> Place {
        let find = |key| self.options.iter().find(|written| written.key == key);
        let model = match key {
            OptionKey::Model { element, .. } => self.models.get(element).copied(),
            // An option of the `ibis` block that is not written: the block.
            OptionKey::Ibis(_) => find(OptionKey::IbisBlock).map(|w| w.value),
            _ => None,
        };
        let written = find(key);
        let entry = written.and_then(|w| w.entries.get(entry?).copied());
        entry
            .or(written.map(|w| w.value.into()))
            .or(model.map(Place::from))
            .or_else(|| find(fallback).map(|w| w.value.into()))
            .unwrap_or(default.into())
    }
}

/// What a key sets, if errors should point at it: the option, and the places of its entries.
pub(crate) type Recorded = Option<(OptionKey, Vec<Place>)>;

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

/// The options shared by both macros.
#[derive(Clone, Copy)]
enum CommonKey {
    Value,
    RefPrefix,
    Pins,
    InternalConnections,
    SimModel,
}

/// The options of `kicad_part!` only.
#[derive(Clone, Copy)]
enum KicadKey {
    Symbol,
    Footprint,
    ExtraPads,
    Spice,
    Sim,
    Ibis,
    Fields,
    SymbolDir,
    FootprintDir,
    SimLibraryDir,
}

/// A key of `kicad_part!`.
#[derive(Clone, Copy)]
enum KicadPartKey {
    Common(CommonKey),
    Kicad(KicadKey),
}

/// A key of `spice_part!`.
#[derive(Clone, Copy)]
enum SpicePartKey {
    Common(CommonKey),
    Model(BindingField),
}

/// The keys of each macro (and of the `sim` block), in the order the error message lists them.
const KICAD_KEYS: &[(&str, KicadPartKey)] = &[
    ("symbol", KicadPartKey::Kicad(KicadKey::Symbol)),
    ("footprint", KicadPartKey::Kicad(KicadKey::Footprint)),
    ("value", KicadPartKey::Common(CommonKey::Value)),
    ("ref_prefix", KicadPartKey::Common(CommonKey::RefPrefix)),
    ("pins", KicadPartKey::Common(CommonKey::Pins)),
    (
        "internal_connections",
        KicadPartKey::Common(CommonKey::InternalConnections),
    ),
    ("extra_pads", KicadPartKey::Kicad(KicadKey::ExtraPads)),
    ("spice", KicadPartKey::Kicad(KicadKey::Spice)),
    ("sim", KicadPartKey::Kicad(KicadKey::Sim)),
    ("ibis", KicadPartKey::Kicad(KicadKey::Ibis)),
    ("sim_model", KicadPartKey::Common(CommonKey::SimModel)),
    ("fields", KicadPartKey::Kicad(KicadKey::Fields)),
    ("symbol_dir", KicadPartKey::Kicad(KicadKey::SymbolDir)),
    ("footprint_dir", KicadPartKey::Kicad(KicadKey::FootprintDir)),
    (
        "sim_library_dir",
        KicadPartKey::Kicad(KicadKey::SimLibraryDir),
    ),
];
const SPICE_KEYS: &[(&str, SpicePartKey)] = &[
    ("library", SpicePartKey::Model(BindingField::Library)),
    ("name", SpicePartKey::Model(BindingField::Name)),
    ("value", SpicePartKey::Common(CommonKey::Value)),
    ("ref_prefix", SpicePartKey::Common(CommonKey::RefPrefix)),
    ("pins", SpicePartKey::Common(CommonKey::Pins)),
    (
        "internal_connections",
        SpicePartKey::Common(CommonKey::InternalConnections),
    ),
    ("params", SpicePartKey::Model(BindingField::Params)),
    ("dialect", SpicePartKey::Model(BindingField::Dialect)),
    ("sim_model", SpicePartKey::Common(CommonKey::SimModel)),
];
const SIM_KEYS: &[(&str, BindingField)] = &[
    ("library", BindingField::Library),
    ("name", BindingField::Name),
    ("pins", BindingField::Pins),
    ("unused", BindingField::Unused),
    ("params", BindingField::Params),
    ("dialect", BindingField::Dialect),
    ("suffix", BindingField::Suffix),
];

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
        let content;
        braced!(content in input);
        parse_keys(
            &content,
            KICAD_KEYS,
            &[("symbol", "`symbol: \"Library:Name\"` is required")],
            header.name.span(),
            |key, content, span| {
                let recorded = match key {
                    KicadPartKey::Common(key) => common_key(key, content, &mut options.common)?,
                    KicadPartKey::Kicad(KicadKey::Symbol) => {
                        options.symbol = content.parse::<LitStr>()?.value();
                        Some((OptionKey::Symbol, Vec::new()))
                    }
                    KicadPartKey::Kicad(KicadKey::Footprint) => {
                        options.footprint = if content.peek(LitStr) {
                            FootprintOption::Given(content.parse::<LitStr>()?.value())
                        } else {
                            keyword(content, &["none"])?;
                            FootprintOption::None
                        };
                        Some((OptionKey::Footprint, Vec::new()))
                    }
                    KicadPartKey::Kicad(KicadKey::Fields) => {
                        options.fields = fields(content)?;
                        None
                    }
                    KicadPartKey::Kicad(KicadKey::ExtraPads) => {
                        options.extra_pads = string_list(content)?.0;
                        Some((OptionKey::ExtraPads, Vec::new()))
                    }
                    KicadPartKey::Kicad(KicadKey::Spice) => {
                        options.spice =
                            match keyword(content, &["auto", "none", "excluded"])?.as_str() {
                                "auto" => SpiceOption::Auto,
                                "none" => SpiceOption::None,
                                _ => SpiceOption::Excluded,
                            };
                        Some((OptionKey::Spice, Vec::new()))
                    }
                    KicadPartKey::Kicad(KicadKey::Sim) => {
                        options.sim = sim_block(content, &mut sim_spans)?;
                        None
                    }
                    KicadPartKey::Kicad(KicadKey::Ibis) => {
                        options.ibis = Some(ibis_block(content, span, &mut sim_spans)?);
                        Some((OptionKey::IbisBlock, Vec::new()))
                    }
                    KicadPartKey::Kicad(KicadKey::SymbolDir) => {
                        symbol_dir = Some(content.parse()?);
                        None
                    }
                    KicadPartKey::Kicad(KicadKey::FootprintDir) => {
                        footprint_dir = Some(content.parse()?);
                        None
                    }
                    KicadPartKey::Kicad(KicadKey::SimLibraryDir) => {
                        sim_library_dir = Some(content.parse()?);
                        None
                    }
                };
                Ok(recorded)
            },
            &mut spans,
        )?;
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
            SPICE_KEYS,
            &[
                ("library", "`library: \"path\"` is required"),
                ("name", "`name: \"NAME\"` is required"),
            ],
            header.name.span(),
            |key, content, _| {
                let recorded = match key {
                    SpicePartKey::Common(key) => common_key(key, content, &mut options.common)?,
                    SpicePartKey::Model(field) => {
                        let entries = model_key(field, content, &mut options.model)?;
                        Some((OptionKey::Model { element: 0, field }, entries))
                    }
                };
                Ok(recorded)
            },
            &mut spans,
        )?;
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
        let block = next_span(input);
        let content;
        braced!(content in input);
        parse_keys(
            &content,
            SIM_KEYS,
            &[
                ("library", "`library` is required in `sim`"),
                ("name", "`name` is required in `sim`"),
            ],
            block,
            |field, content, _| {
                let entries = model_key(field, content, &mut binding)?;
                Ok(Some((OptionKey::Model { element, field }, entries)))
            },
            spans,
        )?;
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

/// `ibis: { library: "...", pin: "...", ... }`.
fn ibis_block(input: ParseStream, block: Span, spans: &mut Spans) -> syn::Result<IbisSource> {
    let mut source = IbisSource::default();
    let content;
    braced!(content in input);
    parse_keys(
        &content,
        IBIS_KEYS,
        &[
            ("library", "`library` is required in `ibis`"),
            ("pin", "`pin` is required in `ibis`"),
        ],
        block,
        |field, content, _| {
            match field {
                IbisField::Library => source.library = content.parse::<LitStr>()?.value(),
                IbisField::Component => source.component = Some(content.parse::<LitStr>()?.value()),
                IbisField::Pin => source.pin = content.parse::<LitStr>()?.value(),
                IbisField::Model => source.model = Some(content.parse::<LitStr>()?.value()),
                IbisField::Diff => source.diff = content.parse::<syn::LitBool>()?.value,
                IbisField::Driver => source.driver = content.parse::<syn::LitBool>()?.value,
                IbisField::Symbol => unreachable!("not a key"),
            }
            Ok(Some((OptionKey::Ibis(field), Vec::new())))
        },
        spans,
    )?;
    Ok(source)
}

const IBIS_KEYS: &[(&str, IbisField)] = &[
    ("library", IbisField::Library),
    ("component", IbisField::Component),
    ("pin", IbisField::Pin),
    ("model", IbisField::Model),
    ("diff", IbisField::Diff),
    ("driver", IbisField::Driver),
];

/// Parses `key: value` pairs separated by commas, with the keys of `keys`. `handle` is given the
/// span of the value; it parses the value and returns the option it sets (see [`Recorded`]). A
/// key of `required` that is not given is an error at `missing`, with the message given with the
/// key.
pub(crate) fn parse_keys<K: Copy>(
    content: ParseStream,
    keys: &[(&str, K)],
    required: &[(&str, &str)],
    missing: Span,
    mut handle: impl FnMut(K, ParseStream, Span) -> syn::Result<Recorded>,
    spans: &mut Spans,
) -> syn::Result<()> {
    let mut seen: Vec<String> = Vec::new();
    comma_separated(content, |content| {
        let key: Ident = content.parse()?;
        let key_name = key.to_string();
        let Some(&(_, handled)) = keys.iter().find(|(name, _)| key == name) else {
            let names: Vec<&str> = keys.iter().map(|(name, _)| *name).collect();
            return Err(syn::Error::new(
                key.span(),
                format!("unknown key `{key}`; expected one of: {}", names.join(", ")),
            ));
        };
        if seen.contains(&key_name) {
            return Err(syn::Error::new(
                key.span(),
                format!("`{key_name}` is given twice"),
            ));
        }
        seen.push(key_name);
        content.parse::<Token![:]>()?;
        let value = next_span(content);
        if let Some((key, entries)) = handle(handled, content, value)? {
            spans.options.push(Written {
                key,
                value,
                entries,
            });
        }
        Ok(())
    })?;
    match required
        .iter()
        .find(|(key, _)| !seen.iter().any(|s| s == key))
    {
        Some((_, message)) => Err(syn::Error::new(missing, *message)),
        None => Ok(()),
    }
}

/// The span of the next token tree: a group's covers its brackets and contents.
fn next_span(input: ParseStream) -> Span {
    input
        .cursor()
        .token_tree()
        .map_or_else(|| input.span(), |(tree, _)| tree.span())
}

/// Calls `item` for each item of the comma-separated list filling `input`; a trailing comma is
/// allowed.
pub(crate) fn comma_separated(
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

/// Parses an option shared by both macros.
fn common_key(
    key: CommonKey,
    content: ParseStream,
    options: &mut CommonOptions,
) -> syn::Result<Recorded> {
    Ok(match key {
        CommonKey::Value => {
            options.value = Some(if content.peek(LitStr) {
                Value::Fixed(content.parse::<LitStr>()?.value())
            } else {
                keyword(content, &["param"])?;
                Value::Param
            });
            Some((OptionKey::Value, Vec::new()))
        }
        CommonKey::RefPrefix => {
            options.ref_prefix = Some(content.parse::<LitStr>()?.value());
            Some((OptionKey::RefPrefix, Vec::new()))
        }
        CommonKey::Pins => {
            let entries;
            (options.pins, entries) = string_map(content)?;
            Some((OptionKey::Pins, entries))
        }
        CommonKey::InternalConnections => {
            let groups;
            bracketed!(groups in content);
            let mut places = Vec::new();
            options.internal_connections.clear();
            comma_separated(&groups, |group| {
                places.push(next_span(group).into());
                options.internal_connections.push(string_list(group)?.0);
                Ok(())
            })?;
            Some((OptionKey::InternalConnections, places))
        }
        CommonKey::SimModel => {
            options.sim_model = match keyword(content, &["include", "inline"])?.as_str() {
                "include" => ModelMode::Include,
                _ => ModelMode::Inline,
            };
            None
        }
    })
}

/// Parses an option of a model (`spice_part!` and the `sim` block); returns the places of its
/// entries.
fn model_key(
    field: BindingField,
    content: ParseStream,
    binding: &mut ModelBinding,
) -> syn::Result<Vec<Place>> {
    let mut entries = Vec::new();
    match field {
        BindingField::Library => binding.library = content.parse::<LitStr>()?.value(),
        BindingField::Name => binding.name = content.parse::<LitStr>()?.value(),
        BindingField::Pins => (binding.pins, entries) = string_map(content)?,
        BindingField::Unused => (binding.unused, entries) = string_list(content)?,
        BindingField::Params => (binding.params, entries) = string_map(content)?,
        BindingField::Dialect => {
            binding.dialect =
                match keyword(content, &["auto", "ngspice", "pspice", "ltspice"])?.as_str() {
                    "ngspice" => DialectOption::Given(Dialect::Ngspice),
                    "pspice" => DialectOption::Given(Dialect::Pspice),
                    "ltspice" => DialectOption::Given(Dialect::Ltspice),
                    _ => DialectOption::Auto,
                }
        }
        BindingField::Suffix => binding.suffix = content.parse::<LitStr>()?.value(),
    }
    Ok(entries)
}

pub(crate) fn end(input: ParseStream) -> syn::Result<()> {
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

/// `["a", "b"]`, with the places of the items.
fn string_list(input: ParseStream) -> syn::Result<(Vec<String>, Vec<Place>)> {
    let content;
    bracketed!(content in input);
    let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
    Ok((
        items.iter().map(LitStr::value).collect(),
        items.iter().map(|item| item.span().into()).collect(),
    ))
}

/// The pairs of a map.
type Pairs = Vec<(String, String)>;

/// `{ "a" => "b", ... }` with distinct keys, with the places of the entries.
fn string_map(input: ParseStream) -> syn::Result<(Pairs, Vec<Place>)> {
    let entries = entries(input)?;
    let places = entries
        .iter()
        .map(|e| Place::new(e.key.span(), e.value.span()))
        .collect();
    Ok((pairs(entries), places))
}

fn pairs(entries: Vec<Entry>) -> Pairs {
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
fn fields(input: ParseStream) -> syn::Result<Pairs> {
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
