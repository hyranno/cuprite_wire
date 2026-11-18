//! Procedural macros generating cuprite_wire parts from KiCad libraries.
//!
//! Use them through the `cuprite_wire` crate (`cuprite_wire::kicad_part!`), which documents them.

mod input;

use std::path::PathBuf;

use cuprite_wire_kicad_lib::{
    part::{OptionKey, PartDef, load_part},
    resolve::Resolver,
    sim::{Args, ElementDef, ModelDef, SpiceDef, Value},
    spice_lib::Dialect,
};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};

/// Generates a part type (a struct implementing `Part`) from a KiCad symbol and footprint.
///
/// The libraries are read at compile time; the generated code does not read them at run time.
///
/// ```ignore
/// cuprite_wire::kicad_part! {
///     /// A dual op-amp in SOIC-8.
///     pub struct Lm358Soic {
///         symbol: "Amplifier_Operational:LM358",
///         footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
///     }
/// }
///
/// cuprite_wire::kicad_part! {
///     pub struct Resistor0603 {
///         symbol: "Device:R",
///         footprint: "Resistor_SMD:R_0603_1608Metric",
///         value: param, // `Resistor0603::new("10k")`
///     }
/// }
/// ```
///
/// Keys (only `symbol` is required):
///
/// | Key | Value | Default |
/// |---|---|---|
/// | `symbol` | `"Library:Name"` | |
/// | `footprint` | `"Library:Name"` or `none` | the symbol's `Footprint` property |
/// | `value` | `"text"` or `param` (a constructor argument) | the symbol's `Value` |
/// | `ref_base` | reference prefix | the symbol's `Reference` |
/// | `pins` | `{ "1" => "out_a", ... }`: pin IDs by pin number | the pin numbers |
/// | `internal_connections` | `[["4", "ep"], ...]`: groups of pin IDs | stacked pins |
/// | `extra_pads` | `["9"]`: footprint pads without a symbol pin, made pins | none (such pads are an error) |
/// | `spice` | `auto`, `none` (not simulated) or `excluded` (`exclude_from_sim`) | `auto` (from `Sim.*`) |
/// | `sim_model` | `include` or `inline` (copy model definitions into the code) | `include` |
/// | `fields` | `{ "MPN" => "..." }`: fields of the footprint | none |
/// | `symbol_dir`, `footprint_dir` | library directories, relative to the crate | `$KICAD9_SYMBOL_DIR` / `$KICAD9_FOOTPRINT_DIR`, else `/usr/share/kicad/...` |
/// | `sim_library_dir` | base of relative `Sim.Library` paths, relative to the crate | the crate |
///
/// See `doc/kicad_part_macro.md` for details.
#[proc_macro]
pub fn kicad_part(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as input::PartInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(input: &input::PartInput) -> syn::Result<TokenStream> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| syn::Error::new(Span::call_site(), "CARGO_MANIFEST_DIR is not set"))?;
    let mut resolver = Resolver::new(manifest_dir, |name| std::env::var(name).ok());
    resolver.symbol_dir = input.symbol_dir.as_ref().map(|s| PathBuf::from(s.value()));
    resolver.footprint_dir = input.footprint_dir.as_ref().map(|s| PathBuf::from(s.value()));
    resolver.sim_library_dir = input.sim_library_dir.as_ref().map(|s| PathBuf::from(s.value()));
    let result = load_part(&input.options, &mut resolver);
    // Track what was read even when it failed, so fixing a library file triggers a rebuild.
    let tracking = tracking(&resolver);
    let part = match result {
        Ok(part) => part,
        Err(error) => {
            let span = input.span_of(error.key);
            let error = syn::Error::new(span, error.error.message).to_compile_error();
            return Ok(quote! { #error #tracking });
        }
    };
    Ok(generate(input, &part, tracking))
}

/// Makes the compiler rebuild when a file read or an environment variable consulted changes.
fn tracking(resolver: &Resolver) -> TokenStream {
    let files = resolver
        .files
        .iter()
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned());
    let vars = &resolver.env_vars;
    quote! {
        const _: () = {
            #( const _: &[u8] = include_bytes!(#files); )*
            #( const _: Option<&str> = option_env!(#vars); )*
        };
    }
}

fn generate(input: &input::PartInput, part: &PartDef, tracking: TokenStream) -> TokenStream {
    let krate = quote!(::cuprite_wire);
    let attrs = &input.attrs;
    let vis = &input.vis;
    let name = &input.name;
    let doc = doc_lines(part);

    let pin_ids: Vec<&str> = part.pins.iter().map(|pin| pin.id.as_str()).collect();
    // Constants for pins given an ID that is a valid identifier (e.g. `out_a` -> `OUT_A`).
    let renamed: Vec<&str> = input
        .options
        .pins
        .iter()
        .map(|(_, id)| id.as_str())
        .filter(|id| is_identifier(id))
        .collect();
    let pin_consts = renamed.iter().map(|id| {
        let ident = format_ident!("{}", id.to_ascii_uppercase());
        let doc = format!("The pin ID `{id}`.");
        quote! { #[doc = #doc] pub const #ident: &'static str = #id; }
    });

    let (fields, constructor, value_fn) = match &part.value {
        Value::Param => (
            quote!({ value: String }),
            quote! {
                /// Creates the part with the given value (e.g. `"10k"`).
                pub fn new(value: impl Into<String>) -> Self {
                    #name { value: value.into() }
                }
            },
            quote!(self.value.clone()),
        ),
        Value::Fixed(value) => (
            quote!(;),
            quote! {
                pub const fn new() -> Self {
                    #name
                }
            },
            quote!(#value.to_string()),
        ),
    };
    let default_impl = matches!(part.value, Value::Fixed(_)).then(|| {
        quote! {
            impl ::core::default::Default for #name {
                fn default() -> Self {
                    #name
                }
            }
        }
    });

    let ref_base = &part.ref_base;
    let groups = part.internal_connections.iter().map(|group| quote!(vec![#(#group.to_string()),*]));

    let footprint = match &part.footprint {
        Some(id) => {
            let (library, name) = (&id.library, &id.name);
            quote!(Some(#krate::kicad::LibId::new(#library, #name).unwrap()))
        }
        None => quote!(None),
    };
    let (symbol_library, symbol_name) = (&part.symbol.library, &part.symbol.name);
    let pin_entries = part.pins.iter().map(|pin| {
        let id = &pin.id;
        let pad = &pin.pad;
        let pin_name = match &pin.name {
            Some(n) => quote!(Some(#n)),
            None => quote!(None),
        };
        let pin_type = format_ident!("{}", pin_type_variant(pin.electrical_type.as_str()));
        quote! {
            (#id.to_string(), #krate::kicad::KicadPinInfo::new(#pad, #pin_name, #krate::kicad::PinType::#pin_type))
        }
    });
    let field_entries = part
        .fields
        .iter()
        .map(|(key, value)| quote!((#key.to_string(), #value.to_string())));
    let (exclude_from_bom, exclude_from_board) = (part.exclude_from_bom, part.exclude_from_board);

    let spice_fn = spice_fn(part, &krate);

    quote! {
        #(#attrs)*
        #(#[doc = #doc])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        #vis struct #name #fields

        impl #name {
            /// The pin IDs of the part.
            pub const PINS: &'static [&'static str] = &[#(#pin_ids),*];
            #(#pin_consts)*
            #constructor
        }

        #default_impl

        impl #krate::parts::Part for #name {
            fn get_ref_base_name(&self) -> String {
                #ref_base.to_string()
            }
            fn pin_ids(&self) -> Vec<String> {
                Self::PINS.iter().map(|id| id.to_string()).collect()
            }
            fn value(&self) -> String {
                #value_fn
            }
            fn internal_connections(&self) -> Vec<Vec<String>> {
                vec![#(#groups),*]
            }
            fn kicad(&self) -> Option<#krate::kicad::KicadPartInfo> {
                Some(#krate::kicad::KicadPartInfo {
                    footprint: #footprint,
                    pins: [#(#pin_entries),*].into_iter().collect(),
                    lib_source: Some(#krate::kicad::LibId::new(#symbol_library, #symbol_name).unwrap()),
                    fields: vec![#(#field_entries),*],
                    flags: #krate::kicad::PartFlags {
                        dnp: false,
                        exclude_from_bom: #exclude_from_bom,
                        exclude_from_board: #exclude_from_board,
                    },
                })
            }
            #spice_fn
        }

        #tracking
    }
}

fn spice_fn(part: &PartDef, krate: &TokenStream) -> Option<TokenStream> {
    let body = match &part.spice {
        SpiceDef::Unsupported => return None,
        SpiceDef::Excluded => quote!(#krate::spice::SpicePartInfo::Excluded),
        SpiceDef::Elements { elements, unused_pins } => {
            let elements = elements.iter().map(|element| element_tokens(element, krate));
            let unused = (!unused_pins.is_empty()).then(|| quote!(.with_unused_pins([#(#unused_pins),*])));
            quote! {
                #krate::spice::SpicePartInfo::elements([#(#elements),*])
                #unused
            }
        }
    };
    Some(quote! {
        fn spice(&self) -> Option<#krate::spice::SpicePartInfo> {
            Some(#body)
        }
    })
}

fn element_tokens(element: &ElementDef, krate: &TokenStream) -> TokenStream {
    let device = element.device;
    let nodes = &element.nodes;
    let args = match &element.args {
        Args::Fixed(text) => quote!(#text),
        Args::Value { prefix } => quote!(format!("{}{}", #prefix, self.value)),
    };
    let dialect = match element.dialect {
        Dialect::Ngspice => None,
        Dialect::Pspice => Some(quote!(.pspice())),
        Dialect::Ltspice => Some(quote!(.ltspice())),
    };
    let models = element.models.iter().map(|model| match model {
        ModelDef::Include(path) => {
            let path = path.to_string_lossy();
            quote!(.with_model(#krate::spice::ModelSource::include(#path) #dialect))
        }
        ModelDef::Inline { name, text } => {
            quote!(.with_model(#krate::spice::ModelSource::inline(#name, #text) #dialect))
        }
    });
    let suffix = &element.suffix;
    let suffix = (!suffix.is_empty()).then(|| quote!(.with_suffix(#suffix)));
    quote!(#krate::spice::SpiceElement::new(#device, [#(#nodes),*], #args) #suffix #(#models)*)
}

/// Documentation appended to the user's: where the part comes from and its pins.
fn doc_lines(part: &PartDef) -> Vec<String> {
    let mut lines = vec![String::new()];
    let footprint = part
        .footprint
        .as_ref()
        .map_or("no footprint".to_string(), |id| format!("footprint `{id}`"));
    lines.push(format!("Generated from the KiCad symbol `{}` with {footprint}.", part.symbol));
    if let Some(description) = &part.description {
        lines.push(String::new());
        lines.push(format!("Symbol description: {}", description.replace('\n', " ")));
    }
    if let Some(datasheet) = &part.datasheet {
        lines.push(String::new());
        lines.push(format!("Datasheet: <{datasheet}>"));
    }
    lines.push(String::new());
    lines.push("| Pin ID | Pad | Name | Type | Unit |".into());
    lines.push("|---|---|---|---|---|".into());
    for pin in &part.pins {
        let unit = if pin.unit == 0 { "-".to_string() } else { pin.unit.to_string() };
        lines.push(format!(
            "| `{}` | {} | {} | {} | {} |",
            pin.id,
            pin.pad,
            pin.name.as_deref().map_or("".into(), |n| format!("`{}`", n.replace('|', "\\|"))),
            pin.electrical_type.as_str(),
            unit
        ));
    }
    lines.into_iter().map(|line| format!(" {line}")).collect()
}

fn pin_type_variant(electrical_type: &str) -> String {
    electrical_type
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn is_identifier(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && text != "_"
        && syn::parse_str::<syn::Ident>(&text.to_ascii_uppercase()).is_ok()
        && text.to_ascii_uppercase() != "PINS"
}

impl input::PartInput {
    fn span_of(&self, key: OptionKey) -> Span {
        self.spans
            .iter()
            .find(|(k, _)| *k == key)
            .or_else(|| self.spans.iter().find(|(k, _)| *k == OptionKey::Symbol))
            .map_or(self.name.span(), |(_, span)| *span)
    }
}
