//! Parsing the input of `kicad_part!`.

use cuprite_wire_kicad_lib::{
    part::{FootprintOption, OptionKey, PartOptions, SpiceOption},
    sim::{ModelMode, Value},
};
use proc_macro2::Span;
use syn::{
    Attribute, Ident, LitStr, Token, Visibility, braced, bracketed,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

/// `#[attrs] vis struct Name { key: value, ... }`
pub struct PartInput {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub name: Ident,
    pub options: PartOptions,
    pub symbol_dir: Option<LitStr>,
    pub footprint_dir: Option<LitStr>,
    pub sim_library_dir: Option<LitStr>,
    /// Where each option is written, to point errors at it.
    pub spans: Vec<(OptionKey, Span)>,
}

const KEYS: &str = "symbol, footprint, value, ref_base, pins, internal_connections, extra_pads, \
                    spice, sim_model, fields, symbol_dir, footprint_dir, sim_library_dir";

impl Parse for PartInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis: Visibility = input.parse()?;
        input.parse::<Token![struct]>()?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut part = PartInput {
            attrs,
            vis,
            options: PartOptions {
                type_name: name.to_string(),
                ..Default::default()
            },
            name,
            symbol_dir: None,
            footprint_dir: None,
            sim_library_dir: None,
            spans: Vec::new(),
        };
        let mut seen: Vec<String> = Vec::new();
        let mut has_symbol = false;
        while !content.is_empty() {
            let key: Ident = content.parse()?;
            let key_name = key.to_string();
            if seen.contains(&key_name) {
                return Err(syn::Error::new(key.span(), format!("`{key_name}` is given twice")));
            }
            seen.push(key_name.clone());
            content.parse::<Token![:]>()?;
            // The whole value (a group's span covers its brackets and contents).
            let span = content
                .cursor()
                .token_tree()
                .map_or_else(|| content.span(), |(tree, _)| tree.span());
            let options = &mut part.options;
            let mut recorded = None;
            match key_name.as_str() {
                "symbol" => {
                    options.symbol = content.parse::<LitStr>()?.value();
                    has_symbol = true;
                    recorded = Some(OptionKey::Symbol);
                }
                "footprint" => {
                    options.footprint = if content.peek(LitStr) {
                        FootprintOption::Given(content.parse::<LitStr>()?.value())
                    } else {
                        keyword(&content, &["none"])?;
                        FootprintOption::None
                    };
                    recorded = Some(OptionKey::Footprint);
                }
                "value" => {
                    options.value = Some(if content.peek(LitStr) {
                        Value::Fixed(content.parse::<LitStr>()?.value())
                    } else {
                        keyword(&content, &["param"])?;
                        Value::Param
                    });
                    recorded = Some(OptionKey::Value);
                }
                "ref_base" => {
                    options.ref_base = Some(content.parse::<LitStr>()?.value());
                    recorded = Some(OptionKey::RefBase);
                }
                "pins" => {
                    options.pins = string_map(&content)?;
                    recorded = Some(OptionKey::Pins);
                }
                "fields" => options.fields = string_map(&content)?,
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
                    recorded = Some(OptionKey::InternalConnections);
                }
                "extra_pads" => {
                    options.extra_pads = string_list(&content)?;
                    recorded = Some(OptionKey::ExtraPads);
                }
                "spice" => {
                    options.spice = match keyword(&content, &["auto", "none", "excluded"])?.as_str() {
                        "auto" => SpiceOption::Auto,
                        "none" => SpiceOption::None,
                        _ => SpiceOption::Excluded,
                    };
                    recorded = Some(OptionKey::Spice);
                }
                "sim_model" => {
                    options.sim_model = match keyword(&content, &["include", "inline"])?.as_str() {
                        "include" => ModelMode::Include,
                        _ => ModelMode::Inline,
                    };
                }
                "symbol_dir" => part.symbol_dir = Some(content.parse()?),
                "footprint_dir" => part.footprint_dir = Some(content.parse()?),
                "sim_library_dir" => part.sim_library_dir = Some(content.parse()?),
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown key `{key_name}`; expected one of: {KEYS}"),
                    ));
                }
            }
            if let Some(key) = recorded {
                part.spans.push((key, span));
            }
            if content.is_empty() {
                break;
            }
            content.parse::<Token![,]>()?;
        }
        if !has_symbol {
            return Err(syn::Error::new(part.name.span(), "`symbol: \"Library:Name\"` is required"));
        }
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after the part definition"));
        }
        Ok(part)
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
        Err(syn::Error::new(ident.span(), format!("expected {}", expected.join(" or "))))
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
    Ok(entries.into_iter().map(|e| (e.key.value(), e.value.value())).collect())
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
