//! `ibis_part!`: parsing its input and generating the type.

use cuprite_wire_kicad_lib::{
    ibis_part::{
        IbisKey, IbisPartDef, IbisPartOptions, IbisPinDef, IbisPinField, IbisPinKind,
        IbisPinOption, load_ibis_part,
    },
    resolve::Resolver,
};
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    Ident, LitStr, Token, braced, bracketed,
    parse::{Parse, ParseStream},
};

use crate::input::{Header, Spans, comma_separated, end, parse_keys};

pub struct IbisPartInput {
    pub header: Header,
    pub options: IbisPartOptions,
    spans: IbisSpans,
    /// The function names, in the order of `options.pins`.
    names: Vec<Ident>,
}

/// Where each option is written, to point errors at it.
#[derive(Default)]
struct IbisSpans {
    library: Option<Span>,
    component: Option<Span>,
    /// Of each entry of the pins: the function name, the pin, the second pin and the model.
    pins: Vec<PinSpans>,
}

struct PinSpans {
    name: Span,
    pin: Span,
    pin2: Option<Span>,
    model: Option<Span>,
}

impl IbisSpans {
    fn of(&self, key: IbisKey, default: Span) -> Span {
        match key {
            IbisKey::Library => self.library,
            IbisKey::Component => self.component,
            IbisKey::Pin { index, field } => self.pins.get(index).map(|p| match field {
                IbisPinField::Pin => p.pin,
                IbisPinField::Pin2 => p.pin2.unwrap_or(p.pin),
                IbisPinField::Model => p.model.unwrap_or(p.name),
            }),
        }
        .unwrap_or(default)
    }
}

#[derive(Clone, Copy)]
enum Key {
    Library,
    Component,
    Pins,
    DiffPins,
    Series,
}

const KEYS: &[(&str, Key)] = &[
    ("library", Key::Library),
    ("component", Key::Component),
    ("pins", Key::Pins),
    ("diff_pins", Key::DiffPins),
    ("series", Key::Series),
];

/// The names the generated type uses for itself.
const RESERVED: &[&str] = &["file"];

impl Parse for IbisPartInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let header: Header = input.parse()?;
        let mut options = IbisPartOptions::default();
        let mut spans = IbisSpans::default();
        let mut names: Vec<Ident> = Vec::new();
        let content;
        braced!(content in input);
        parse_keys(
            &content,
            KEYS,
            |key, content, span| {
                match key {
                    Key::Library => {
                        options.library = content.parse::<LitStr>()?.value();
                        spans.library = Some(span);
                    }
                    Key::Component => {
                        options.component = Some(content.parse::<LitStr>()?.value());
                        spans.component = Some(span);
                    }
                    Key::Pins | Key::DiffPins | Key::Series => {
                        let entries;
                        braced!(entries in content);
                        comma_separated(&entries, |entries| {
                            let (option, pin_spans, name) = entry(entries, key)?;
                            if let Some(first) = names.iter().find(|n| **n == name) {
                                let mut error = syn::Error::new(
                                    name.span(),
                                    format!("the function `{name}` is defined twice"),
                                );
                                error.combine(syn::Error::new(first.span(), "first defined here"));
                                return Err(error);
                            }
                            if RESERVED.contains(&name.to_string().as_str()) {
                                return Err(syn::Error::new(
                                    name.span(),
                                    format!(
                                        "`{name}` is the name of the function returning the file"
                                    ),
                                ));
                            }
                            names.push(name);
                            options.pins.push(option);
                            spans.pins.push(pin_spans);
                            Ok(())
                        })?;
                    }
                }
                Ok(None)
            },
            &mut Spans::default(),
        )?;
        if spans.library.is_none() {
            return Err(syn::Error::new(
                header.name.span(),
                "`library: \"path/to/file.ibs\"` is required",
            ));
        }
        end(input)?;
        Ok(IbisPartInput {
            header,
            options,
            spans,
            names,
        })
    }
}

/// `name => "pin"`, `name => { pin: "pin", model: "model" }` (pins and diff_pins) or
/// `name => ["pin1", "pin2"]` (series).
fn entry(input: ParseStream, key: Key) -> syn::Result<(IbisPinOption, PinSpans, Ident)> {
    let name: Ident = input.parse()?;
    input.parse::<Token![=>]>()?;
    let kind_of = |pin2: Option<&LitStr>| match (key, pin2) {
        (Key::Series, Some(pin2)) => IbisPinKind::Series(pin2.value()),
        (Key::DiffPins, _) => IbisPinKind::Diff,
        _ => IbisPinKind::Single,
    };
    let (pin, pin2, model) = if let Key::Series = key {
        let content;
        bracketed!(content in input);
        let pin: LitStr = content.parse()?;
        content.parse::<Token![,]>()?;
        let pin2: LitStr = content.parse()?;
        content.parse::<Option<Token![,]>>()?;
        if !content.is_empty() {
            return Err(content.error("a series model joins two pins: `[\"pin1\", \"pin2\"]`"));
        }
        (pin, Some(pin2), None)
    } else if input.peek(LitStr) {
        (input.parse()?, None, None)
    } else if input.peek(syn::token::Brace) {
        let content;
        braced!(content in input);
        let (mut pin, mut model) = (None, None);
        parse_keys(
            &content,
            &[("pin", true), ("model", false)],
            |is_pin, content, _| {
                let value: LitStr = content.parse()?;
                if is_pin {
                    pin = Some(value)
                } else {
                    model = Some(value)
                }
                Ok(None)
            },
            &mut Spans::default(),
        )?;
        let pin = pin.ok_or_else(|| syn::Error::new(name.span(), "`pin: \"...\"` is required"))?;
        (pin, None, model)
    } else {
        return Err(input.error("expected a pin `\"1\"` or `{ pin: \"1\", model: \"...\" }`"));
    };
    let option = IbisPinOption {
        name: name.to_string(),
        kind: kind_of(pin2.as_ref()),
        pin: pin.value(),
        model: model.as_ref().map(LitStr::value),
    };
    let spans = PinSpans {
        name: name.span(),
        pin: pin.span(),
        pin2: pin2.as_ref().map(LitStr::span),
        model: model.as_ref().map(LitStr::span),
    };
    Ok((option, spans, name))
}

pub fn expand(input: &IbisPartInput, resolver: &mut Resolver) -> TokenStream {
    let result = load_ibis_part(&input.options, resolver);
    // Track what was read even when it failed, so fixing the file triggers a rebuild.
    let tracking = crate::tracking(resolver);
    match result {
        Ok(part) => generate(&input.header, &input.names, &part, tracking),
        Err(error) => {
            let span = input.spans.of(error.key, input.header.name.span());
            let error = syn::Error::new(span, error.error.message).to_compile_error();
            quote! { #error #tracking }
        }
    }
}

fn generate(
    header: &Header,
    names: &[Ident],
    part: &IbisPartDef,
    tracking: TokenStream,
) -> TokenStream {
    let krate = quote!(::cuprite_wire);
    let attrs = &header.attrs;
    let vis = &header.vis;
    let name = &header.name;
    let doc = doc_lines(part);
    let file_items = file_items(part, &krate);
    let functions = part
        .pins
        .iter()
        .zip(names)
        .map(|(pin, name)| function(pin, name, &krate));
    quote! {
        #(#attrs)*
        #(#[doc = #doc])*
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::core::marker::Copy,
            ::core::cmp::PartialEq,
            ::core::cmp::Eq,
            ::core::default::Default,
        )]
        #vis struct #name;

        impl #name {
            #file_items

            #(#functions)*
        }

        #tracking
    }
}

/// The constants `LIBRARY` and `COMPONENT`, and the function `file` reading the file once.
pub(crate) fn file_items(part: &IbisPartDef, krate: &TokenStream) -> TokenStream {
    let path = part.path.to_string_lossy();
    let component = &part.component;
    quote! {
            /// The IBIS file (absolute), read at run time.
            pub const LIBRARY: &'static str = #path;
            /// The `[Component]` of the file.
            pub const COMPONENT: &'static str = #component;

            /// The IBIS file, read and parsed at the first call.
            ///
            /// # Errors
            /// If the file cannot be read or parsed.
            pub fn file() -> ::core::result::Result<
                &'static #krate::ibis::IbisFile,
                #krate::spice::ibis::IbisBufferError,
            > {
                static FILE: ::std::sync::OnceLock<
                    ::core::result::Result<
                        #krate::ibis::IbisFile,
                        #krate::spice::ibis::IbisBufferError,
                    >,
                > = ::std::sync::OnceLock::new();
                FILE.get_or_init(|| #krate::spice::ibis::read_file(Self::LIBRARY))
                    .as_ref()
                    .map_err(::core::clone::Clone::clone)
            }
    }
}

/// The function of an entry of the pins.
fn function(pin: &IbisPinDef, name: &Ident, krate: &TokenStream) -> TokenStream {
    let option = &pin.option;
    let p = &option.pin;
    let (ty, call, doc) = match &option.kind {
        IbisPinKind::Single => {
            let buffer = &pin.buffers[0];
            let call = match &option.model {
                Some(model) => {
                    quote!(of_pin_model(Self::file()?, Self::COMPONENT, #p, #model, corner))
                }
                None => quote!(of_pin(Self::file()?, Self::COMPONENT, #p, corner)),
            };
            let doc = format!(
                "The buffer of the pin `{p}` ({}), at `corner`.",
                describe(buffer)
            );
            (quote!(IbisBuffer), call, doc)
        }
        IbisPinKind::Diff => {
            let call = match &option.model {
                Some(model) => {
                    quote!(of_pin_model(Self::file()?, Self::COMPONENT, #p, #model, corner))
                }
                None => quote!(of_pin(Self::file()?, Self::COMPONENT, #p, corner)),
            };
            let pins: Vec<String> = pin
                .buffers
                .iter()
                .map(|b| format!("`{}` ({})", b.pin, describe(b)))
                .collect();
            let termination = pin
                .series
                .as_ref()
                .map(|s| format!(", terminated by the series model `{s}`"))
                .unwrap_or_default();
            let doc = format!(
                "The differential pair of the pins {}{termination}, at `corner`.",
                pins.join(" and ")
            );
            (quote!(IbisDiffBuffer), call, doc)
        }
        IbisPinKind::Series(p2) => {
            let call = quote!(of_pins(Self::file()?, Self::COMPONENT, #p, #p2, corner));
            let doc = format!(
                "The series model `{}` between the pins `{p}` and `{p2}`, at `corner`.",
                pin.series.as_deref().unwrap_or_default()
            );
            (quote!(IbisSeries), call, doc)
        }
    };
    quote! {
        #[doc = #doc]
        ///
        /// # Errors
        /// If the file cannot be read at run time, or no longer has the pins and models that
        /// were checked when compiling.
        pub fn #name(
            corner: #krate::ibis::Corner,
        ) -> ::core::result::Result<
            #krate::spice::ibis::#ty,
            #krate::spice::ibis::IbisBufferError,
        > {
            #krate::spice::ibis::#ty::#call
        }
    }
}

/// `signal TX, model out_3s (3-state)`, with the selector if any.
pub(crate) fn describe(buffer: &cuprite_wire_kicad_lib::ibis_part::BufferDef) -> String {
    let selector = buffer
        .selector
        .as_ref()
        .map(|s| format!(" of the selector `{s}`"))
        .unwrap_or_default();
    format!(
        "signal `{}`, model `{}`{selector}, {}",
        buffer.signal_name, buffer.model, buffer.model_type
    )
}

/// Documentation appended to the user's: the file and the functions.
fn doc_lines(part: &IbisPartDef) -> Vec<String> {
    let mut lines = vec![String::new()];
    let rev = part
        .file_rev
        .as_ref()
        .map(|r| format!(", revision {r}"))
        .unwrap_or_default();
    let manufacturer = part
        .manufacturer
        .as_ref()
        .map(|m| format!(" by {m}"))
        .unwrap_or_default();
    lines.push(format!(
        "Generated from the component `{}`{manufacturer} of the IBIS file `{}` (IBIS {}{rev}), \
         for simulation. The file is read when a function is first called.",
        part.component,
        part.path.display(),
        part.ibis_ver
    ));
    lines.push(String::new());
    lines.push("| Function | Pins | Signals | Models |".into());
    lines.push("|---|---|---|---|".into());
    for pin in &part.pins {
        let join = |f: &dyn Fn(&cuprite_wire_kicad_lib::ibis_part::BufferDef) -> String| {
            pin.buffers.iter().map(f).collect::<Vec<_>>().join(", ")
        };
        let (pins, signals, mut models) = match &pin.option.kind {
            IbisPinKind::Series(p2) => (
                format!("`{}`, `{p2}`", pin.option.pin),
                String::new(),
                String::new(),
            ),
            _ => (
                join(&|b| format!("`{}`", b.pin)),
                join(&|b| format!("`{}`", b.signal_name)),
                join(&|b| format!("`{}` ({})", b.model, b.model_type)),
            ),
        };
        if let Some(series) = &pin.series {
            if !models.is_empty() {
                models.push_str(", ");
            }
            models.push_str(&format!("`{series}` (Series)"));
        }
        lines.push(format!(
            "| [`{name}`](Self::{name}) | {pins} | {signals} | {models} |",
            name = pin.option.name,
            pins = escape(&pins),
            signals = escape(&signals),
            models = escape(&models),
        ));
    }
    lines.into_iter().map(|line| format!(" {line}")).collect()
}

fn escape(text: &str) -> String {
    text.replace('|', "\\|")
}
