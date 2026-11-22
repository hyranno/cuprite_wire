//! Procedural macros generating cuprite_wire parts from KiCad libraries and SPICE models.
//!
//! Use them through the `cuprite_wire` crate (`cuprite_wire::kicad_part!`,
//! `cuprite_wire::spice_part!`, `#[derive(cuprite_wire::subcircuit::Interface)]`), which documents
//! them.

mod input;
mod interface;

use std::path::PathBuf;

use cuprite_wire_kicad_lib::{
    part::{OptionKey, PartDef, PartError, load_part, load_spice_part},
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
/// | `value` | `"text"` or `param` (a constructor argument) | the symbol's `Value`; text variables such as `${SIM.PARAMS}` are expanded from the fields |
/// | `ref_base` | reference prefix | the symbol's `Reference` |
/// | `pins` | `{ "1" => "out_a", ... }`: pin IDs by pin number | the pin numbers |
/// | `internal_connections` | `[["4", "ep"], ...]`: groups of pin IDs | stacked pins |
/// | `extra_pads` | `["9"]`: footprint pads without a symbol pin, made pins | none (such pads are an error) |
/// | `spice` | `auto`, `none` (not simulated) or `excluded` (`exclude_from_sim`) | `auto` (from `Sim.*`) |
/// | `sim` | a library model replacing the symbol's `Sim.*` fields: `{ library, name, pins: { "1" => "PORT" }, unused, params, dialect }`, or a list of them with `suffix` | the `Sim.*` fields |
/// | `sim_model` | `include` or `inline` (copy model definitions into the code) | `include` |
/// | `fields` | `{ "MPN" => "..." }`: fields of the footprint | none |
/// | `symbol_dir`, `footprint_dir` | library directories, relative to the crate | `$KICAD10_SYMBOL_DIR`, `$KICAD9_SYMBOL_DIR` (and `FOOTPRINT`), else `/usr/share/kicad/...` |
/// | `sim_library_dir` | base of relative `Sim.Library` / `sim.library` paths, relative to the crate | the crate |
///
/// See the guide [Generating parts from KiCad libraries](https://github.com/hyranno/cuprite_wire/blob/main/doc/kicad_part_macro.md) for details.
#[proc_macro]
pub fn kicad_part(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as input::PartInput);
    let mut resolver = match resolver(input.sim_library_dir.as_ref()) {
        Ok(resolver) => resolver,
        Err(error) => return error.to_compile_error().into(),
    };
    resolver.symbol_dir = input.symbol_dir.as_ref().map(|s| PathBuf::from(s.value()));
    resolver.footprint_dir = input
        .footprint_dir
        .as_ref()
        .map(|s| PathBuf::from(s.value()));
    let result = load_part(&input.options, &mut resolver);
    expand(
        &input.header,
        &input.spans,
        OptionKey::Symbol,
        &input.options.common.pins,
        result,
        &resolver,
    )
    .into()
}

/// Generates a simulation-only part type from a `.subckt` or `.model` of a SPICE library.
///
/// ```ignore
/// cuprite_wire::spice_part! {
///     /// A voltage supervisor (simulation only).
///     pub struct Supervisor {
///         library: "models/vsup01.lib",
///         name: "VSUP01",
///         pins: { "RESET_B" => "reset_n" }, // pin IDs are the port names otherwise
///     }
/// }
/// ```
///
/// Keys (`library` and `name` are required):
///
/// | Key | Value | Default |
/// |---|---|---|
/// | `library` | the library file, relative to the crate, or `${VAR}/...` | |
/// | `name` | the `.subckt` or `.model` | |
/// | `pins` | `{ "IN+" => "in_p", ... }`: pin IDs by port name | the port names |
/// | `internal_connections` | `[["a", "b"], ...]`: groups of pin IDs | none |
/// | `params` | `{ "TD" => "1m" }`: parameters of the `.subckt` | the model's defaults |
/// | `value` | `"text"` or `param` | the model name |
/// | `ref_base` | reference prefix | `U` for a `.subckt`; `D` or `Q` for a `.model` |
/// | `dialect` | `auto`, `ngspice`, `pspice` or `ltspice` | `auto` (detected from the library) |
/// | `sim_model` | `include` or `inline` | `include` |
///
/// See the guide [Parts from SPICE models](https://github.com/hyranno/cuprite_wire/blob/main/doc/spice_part_macro.md) for details.
#[proc_macro]
pub fn spice_part(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as input::SpicePartInput);
    let mut resolver = match resolver(None) {
        Ok(resolver) => resolver,
        Err(error) => return error.to_compile_error().into(),
    };
    let result = load_spice_part(&input.options, &mut resolver);
    let name = OptionKey::Model {
        element: 0,
        field: cuprite_wire_kicad_lib::binding::BindingField::Name,
    };
    expand(
        &input.header,
        &input.spans,
        name,
        &input.options.common.pins,
        result,
        &resolver,
    )
    .into()
}

/// Implements `Interface` for a struct with one type parameter whose fields all have that type,
/// e.g. `struct Ports<T = NodeIndex> { input: T, output: T }`. The order of the fields is the
/// order of the interface. `cuprite_wire::subcircuit::Interface` documents it.
#[proc_macro_derive(Interface)]
pub fn derive_interface(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    interface::derive(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn resolver(sim_library_dir: Option<&syn::LitStr>) -> syn::Result<Resolver> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| syn::Error::new(Span::call_site(), "CARGO_MANIFEST_DIR is not set"))?;
    let mut resolver = Resolver::new(manifest_dir, |name| std::env::var(name).ok());
    resolver.sim_library_dir = sim_library_dir.map(|s| PathBuf::from(s.value()));
    Ok(resolver)
}

/// The generated code, or the error pointing at the option concerned (`fallback` if it is not written).
fn expand(
    header: &input::Header,
    spans: &input::Spans,
    fallback: OptionKey,
    renamed: &[(String, String)],
    result: Result<PartDef, PartError>,
    resolver: &Resolver,
) -> TokenStream {
    // Track what was read even when it failed, so fixing a library file triggers a rebuild.
    let tracking = tracking(resolver);
    match result {
        Ok(part) => generate(header, renamed, &part, tracking),
        Err(error) => {
            let span = spans.of(error.key, fallback, header.name.span());
            let error = syn::Error::new(span, error.error.message).to_compile_error();
            quote! { #error #tracking }
        }
    }
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
            #( const _: &[u8] = ::core::include_bytes!(#files); )*
            #( const _: ::core::option::Option<&str> = ::core::option_env!(#vars); )*
        };
    }
}

fn generate(
    header: &input::Header,
    renamed: &[(String, String)],
    part: &PartDef,
    tracking: TokenStream,
) -> TokenStream {
    let krate = quote!(::cuprite_wire);
    let attrs = &header.attrs;
    let vis = &header.vis;
    let name = &header.name;
    let doc = doc_lines(part);

    let pin_ids: Vec<&str> = part.pins.iter().map(|pin| pin.id.as_str()).collect();
    // Constants for pins given an ID that is a valid identifier (e.g. `out_a` -> `OUT_A`).
    let pin_consts = renamed
        .iter()
        .map(|(_, id)| id.as_str())
        .filter(|id| is_identifier(id))
        .map(|id| {
            let ident = format_ident!("{}", id.to_ascii_uppercase());
            let doc = format!("The pin ID `{id}`.");
            quote! { #[doc = #doc] pub const #ident: &'static str = #id; }
        });

    let (fields, constructor, value_fn) = match &part.value {
        Value::Param => {
            // The devices that write the value, which must be a number they read.
            let mut devices: Vec<char> = match &part.spice {
                SpiceDef::Elements { elements, .. } => elements
                    .iter()
                    .filter(|element| matches!(element.args, Args::Value { .. }))
                    .map(|element| element.device)
                    .collect(),
                _ => Vec::new(),
            };
            devices.sort_unstable();
            devices.dedup();
            let (check, panics) = if devices.is_empty() {
                (None, None)
            } else {
                (
                    Some(quote! {
                        #(
                            if let ::core::result::Result::Err(error) =
                                #krate::spice::part::spice_value(#devices, &value)
                            {
                                ::core::panic!("{}", error);
                            }
                        )*
                    }),
                    Some(quote! {
                        ///
                        /// # Panics
                        ///
                        /// If the value is not a number in KiCad's notation (e.g. `"4k7"`, `"1M"`
                        /// for mega), which the simulation needs.
                        #[track_caller]
                    }),
                )
            };
            (
                quote!({ value: ::std::string::String }),
                quote! {
                    /// Creates the part with the given value (e.g. `"10k"`).
                    #panics
                    pub fn new(value: impl ::core::convert::Into<::std::string::String>) -> Self {
                        let value = value.into();
                        #check
                        #name { value }
                    }
                },
                quote!(self.value.clone()),
            )
        }
        Value::Fixed(value) => (
            quote!(;),
            quote! {
                pub const fn new() -> Self {
                    #name
                }
            },
            quote!(::std::string::String::from(#value)),
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
    let groups = part
        .internal_connections
        .iter()
        .map(|group| quote!(::std::vec![#(::std::string::String::from(#group)),*]));
    let kicad_fn = kicad_fn(part, &krate);
    let spice_fn = spice_fn(part, &krate);

    quote! {
        #(#attrs)*
        #(#[doc = #doc])*
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::core::cmp::PartialEq,
            ::core::cmp::Eq,
        )]
        #vis struct #name #fields

        impl #name {
            /// The pin IDs of the part.
            pub const PINS: &'static [&'static str] = &[#(#pin_ids),*];
            #(#pin_consts)*
            #constructor
        }

        #default_impl

        impl #krate::parts::Part for #name {
            fn get_ref_base_name(&self) -> ::std::string::String {
                ::std::string::String::from(#ref_base)
            }
            fn pin_ids(&self) -> ::std::vec::Vec<::std::string::String> {
                Self::PINS.iter().copied().map(::std::string::String::from).collect()
            }
            fn value(&self) -> ::std::string::String {
                #value_fn
            }
            fn internal_connections(&self) -> ::std::vec::Vec<::std::vec::Vec<::std::string::String>> {
                ::std::vec![#(#groups),*]
            }
            #kicad_fn
            #spice_fn
        }

        #tracking
    }
}

fn kicad_fn(part: &PartDef, krate: &TokenStream) -> Option<TokenStream> {
    let kicad = part.kicad.as_ref()?;
    let footprint = match &kicad.footprint {
        Some(id) => {
            let (library, name) = (id.library(), id.name());
            quote!(::core::option::Option::Some(#krate::kicad::LibId::new(#library, #name).unwrap()))
        }
        None => quote!(::core::option::Option::None),
    };
    let (symbol_library, symbol_name) = (kicad.symbol.library(), kicad.symbol.name());
    let pin_entries = part.pins.iter().map(|pin| {
        let id = &pin.id;
        let pad = &pin.pad;
        let pin_name = match &pin.name {
            Some(n) => quote!(::core::option::Option::Some(#n)),
            None => quote!(::core::option::Option::None),
        };
        let pin_type = format_ident!("{}", pin_type_variant(pin.electrical_type.as_str()));
        quote! {
            (::std::string::String::from(#id), #krate::kicad::KicadPinInfo::new(#pad, #pin_name, #krate::kicad::PinType::#pin_type))
        }
    });
    let field_entries = kicad
        .fields
        .iter()
        .map(|(key, value)| quote!((::std::string::String::from(#key), ::std::string::String::from(#value))));
    let (exclude_from_bom, exclude_from_board) = (kicad.exclude_from_bom, kicad.exclude_from_board);
    Some(quote! {
        fn kicad(&self) -> ::core::option::Option<#krate::kicad::KicadPartInfo> {
            ::core::option::Option::Some(#krate::kicad::KicadPartInfo {
                footprint: #footprint,
                pins: [#(#pin_entries),*].into_iter().collect(),
                lib_source: ::core::option::Option::Some(#krate::kicad::LibId::new(#symbol_library, #symbol_name).unwrap()),
                fields: ::std::vec![#(#field_entries),*],
                flags: #krate::kicad::PartFlags {
                    dnp: false,
                    exclude_from_bom: #exclude_from_bom,
                    exclude_from_board: #exclude_from_board,
                },
            })
        }
    })
}

fn spice_fn(part: &PartDef, krate: &TokenStream) -> Option<TokenStream> {
    let body = match &part.spice {
        SpiceDef::Unsupported => return None,
        SpiceDef::Excluded => quote!(#krate::spice::SpicePartInfo::Excluded),
        SpiceDef::Elements {
            elements,
            unused_pins,
        } => {
            let elements = elements
                .iter()
                .map(|element| element_tokens(element, krate));
            let unused =
                (!unused_pins.is_empty()).then(|| quote!(.with_unused_pins([#(#unused_pins),*])));
            quote! {
                #krate::spice::SpicePartInfo::elements([#(#elements),*])
                #unused
            }
        }
    };
    Some(quote! {
        fn spice(&self) -> ::core::option::Option<#krate::spice::SpicePartInfo> {
            ::core::option::Option::Some(#body)
        }
    })
}

fn element_tokens(element: &ElementDef, krate: &TokenStream) -> TokenStream {
    let device = element.device;
    let nodes = &element.nodes;
    let args = match &element.args {
        Args::Fixed(text) => quote!(#text),
        Args::Value { prefix } => quote! {
            ::std::format!(
                "{}{}",
                #prefix,
                // Checked by `new`.
                #krate::spice::part::spice_value(#device, &self.value)
                    .unwrap_or_else(|error| ::core::panic!("{}", error))
            )
        },
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

/// Documentation appended to the user's: where the part comes from, its pins and its model.
fn doc_lines(part: &PartDef) -> Vec<String> {
    let mut lines = vec![String::new()];
    let elements: &[ElementDef] = match &part.spice {
        SpiceDef::Elements { elements, .. } => elements,
        _ => &[],
    };
    let models: Vec<&ElementDef> = elements.iter().filter(|e| e.info.is_some()).collect();
    // The port of each pin in each model.
    let ports_of = |id: &str| -> String {
        let ports: Vec<String> = models
            .iter()
            .filter_map(|element| {
                let index = element.nodes.iter().position(|node| node == id)?;
                let port = escape(&element.info.as_ref()?.ports[index]);
                Some(if element.suffix.is_empty() {
                    format!("`{port}`")
                } else {
                    format!("`{port}` ({})", element.suffix)
                })
            })
            .collect();
        ports.join(", ")
    };
    match &part.kicad {
        Some(kicad) => {
            let footprint = kicad
                .footprint
                .as_ref()
                .map_or("no footprint".to_string(), |id| format!("footprint `{id}`"));
            lines.push(format!(
                "Generated from the KiCad symbol `{}` with {footprint}.",
                kicad.symbol
            ));
            if let Some(description) = &kicad.description {
                lines.push(String::new());
                lines.push(format!(
                    "Symbol description: {}",
                    description.replace('\n', " ")
                ));
            }
            if let Some(datasheet) = &kicad.datasheet {
                lines.push(String::new());
                lines.push(format!("Datasheet: <{datasheet}>"));
            }
            lines.push(String::new());
            let port_column = !models.is_empty();
            lines.push(format!(
                "| Pin ID | Pad | Name | Type | Unit |{}",
                if port_column { " SPICE port |" } else { "" }
            ));
            lines.push(format!(
                "|---|---|---|---|---|{}",
                if port_column { "---|" } else { "" }
            ));
            for pin in &part.pins {
                let unit = if pin.unit == 0 {
                    "-".to_string()
                } else {
                    pin.unit.to_string()
                };
                let port = if port_column {
                    format!(" {} |", ports_of(&pin.id))
                } else {
                    String::new()
                };
                lines.push(format!(
                    "| `{}` | {} | {} | {} | {} |{port}",
                    escape(&pin.id),
                    escape(&pin.pad),
                    pin.name
                        .as_deref()
                        .map_or("".into(), |n| format!("`{}`", escape(n))),
                    pin.electrical_type.as_str(),
                    unit
                ));
            }
        }
        None => {
            lines.push(
                "Generated from a SPICE model, for simulation only (`kicad()` returns `None`)."
                    .into(),
            );
            lines.push(String::new());
            lines.push("| Pin ID | Port |".into());
            lines.push("|---|---|".into());
            for pin in &part.pins {
                lines.push(format!("| `{}` | {} |", escape(&pin.id), ports_of(&pin.id)));
            }
        }
    }
    for element in &models {
        let info = element.info.as_ref().unwrap();
        lines.push(String::new());
        let kind = match &info.model_type {
            Some(model_type) => format!("`.model` of type `{model_type}`"),
            None => "`.subckt`".to_string(),
        };
        let suffix = if element.suffix.is_empty() {
            String::new()
        } else {
            format!(" (element {})", element.suffix)
        };
        lines.push(format!(
            "SPICE model{suffix}: {kind} `{}` in `{}`.",
            info.name,
            info.path.display()
        ));
        lines.push(String::new());
        lines.push(format!(
            "Dialect: {} ({}).",
            info.dialect.as_str(),
            info.dialect_reason
        ));
        if !info.params.is_empty() {
            let params: Vec<String> = info
                .params
                .iter()
                .map(|(k, v)| format!("`{k}={v}`"))
                .collect();
            lines.push(String::new());
            lines.push(format!("Parameters and defaults: {}.", params.join(", ")));
        }
    }
    lines.into_iter().map(|line| format!(" {line}")).collect()
}

/// Escapes `|` in a table cell.
fn escape(text: &str) -> String {
    text.replace('|', "\\|")
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
        && !text.eq_ignore_ascii_case("PINS")
}

#[cfg(test)]
mod tests {
    use cuprite_wire_kicad_lib::{
        part::{KicadDef, PinDef},
        symbol::ElectricalType,
    };

    use super::*;

    /// `|` in any cell of the pin table is escaped, so it does not split the cell.
    #[test]
    fn test_doc_pin_table_escapes_pipes() {
        let part = PartDef {
            kicad: Some(KicadDef {
                symbol: "lib:PART".parse().unwrap(),
                footprint: None,
                fields: Vec::new(),
                exclude_from_bom: false,
                exclude_from_board: false,
                description: None,
                datasheet: None,
            }),
            ref_base: "U".into(),
            value: Value::Fixed("PART".into()),
            pins: vec![PinDef {
                id: "a|b".into(),
                pad: "1|2".into(),
                name: Some("c|d".into()),
                electrical_type: ElectricalType::Passive,
                unit: 0,
            }],
            internal_connections: Vec::new(),
            spice: SpiceDef::Unsupported,
        };
        let lines = doc_lines(&part);
        let row = lines.last().unwrap();
        assert_eq!(row, r" | `a\|b` | 1\|2 | `c\|d` | passive | - |");
    }
}
