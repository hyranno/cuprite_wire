//! The items of a `kicad_part!` type whose symbol is `Sim.Device IBIS` (or that has the `ibis`
//! block): it holds the driver or receiver of `cuprite_wire::spice::ibis` and connects its pins
//! to the symbol's.

use cuprite_wire_kicad_lib::sim::{
    assignments,
    ibis::{DcLevel, IbisDef, IbisInner, Stimulus},
};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

use crate::ibis::{buffer_call, describe, file_items};

/// What the generated type has in place of an ordinary part's.
pub struct IbisItems {
    /// The fields of the struct.
    pub fields: TokenStream,
    /// The items of `impl Name`.
    pub items: TokenStream,
    /// `Part::spice`.
    pub spice_fn: TokenStream,
}

pub fn items(def: &IbisDef, name: &Ident, krate: &TokenStream) -> IbisItems {
    let source = &def.source;
    let IbisInner {
        name: inner,
        buffer,
        pins: inner_pins,
        ..
    } = source.inner();
    let ibis = quote!(#krate::spice::ibis);
    let inner = format_ident!("{inner}");
    let buffer = format_ident!("{buffer}");
    let inner_pins = inner_pins.iter().map(|pin| format_ident!("{pin}"));
    let nodes = &def.nodes;
    let unused = &def.unused_pins;
    let unused =
        (!unused.is_empty()).then(|| quote!(.map(|info| info.with_unused_pins([#(#unused),*]))));

    let pin = &source.pin;
    let call = buffer_call(quote!(Self::IBIS_PIN), source.model.as_ref());
    let warn = (!def.corner_params.is_empty()).then(|| {
        let params = assignments(def.corner_params.iter().map(|(k, v)| (k, v)));
        let type_name = name.to_string();
        quote! {
            static WARNED: ::std::sync::Once = ::std::sync::Once::new();
            WARNED.call_once(|| #ibis::__kicad::warn_corners(#type_name, #params));
        }
    });
    let corner_note = (!def.corner_params.is_empty()).then(|| {
        let params: Vec<String> = def
            .corner_params
            .iter()
            .map(|(k, v)| format!("`{k}={v}`"))
            .collect();
        format!(
            " The symbol's corners per parameter ({}) are not supported: `corner` is used for \
             the whole model, with a warning (through the `log` crate) at the first call.",
            params.join(", ")
        )
    });
    let buffers: Vec<String> = def.part.pins[0]
        .buffers
        .iter()
        .map(|b| format!("`{}` ({})", b.pin, describe(b)))
        .collect();
    let what = if source.diff {
        format!(
            "the differential pair of the pins {}",
            buffers.join(" and ")
        )
    } else {
        format!("the pin {}", buffers[0])
    };
    let new_doc = format!(
        "{} {what} at `corner`.{}",
        if source.driver {
            "Drives `sequence` from"
        } else {
            "Receives with"
        },
        corner_note.unwrap_or_default()
    );
    let (sequence_param, sequence_arg, panics) = if source.driver {
        (
            Some(quote!(, sequence: #krate::spice::digital::DigitalSequence)),
            Some(quote!(, sequence)),
            Some({
                let doc = format!(
                    " If the sequence has a level the model cannot drive (see \
                     [`{inner}::new`](cuprite_wire::spice::ibis::{inner}::new))."
                );
                quote! {
                    ///
                    /// # Panics
                    #[doc = #doc]
                    #[track_caller]
                }
            }),
        )
    } else {
        (None, None, None)
    };
    let sequence_fn = source.driver.then(|| {
        quote! {
            /// The levels the part plays.
            pub fn sequence(&self) -> &#krate::spice::digital::DigitalSequence {
                self.inner.sequence()
            }
        }
    });
    let kicad_sequence = def.stimulus.map(|stimulus| {
        let level = |l: DcLevel| {
            let l = format_ident!(
                "{}",
                match l {
                    DcLevel::High => "High",
                    DcLevel::Low => "Low",
                    DcLevel::HiZ => "HiZ",
                }
            );
            quote!(#krate::spice::digital::Level::#l)
        };
        let (body, doc) = match stimulus {
            Stimulus::Dc(l) => (
                {
                    let l = level(l);
                    quote!(#krate::spice::digital::DigitalSequence::new(#l))
                },
                "the constant level of `Sim.Params` `dc`".to_string(),
            ),
            Stimulus::Rect { ton, toff, td, n } => (
                quote!(#ibis::__kicad::rect_sequence(#ton, #toff, #td, #n)),
                format!(
                    "low, then {n} periods of high for {ton:e} s and low for {toff:e} s from \
                     {td:e} s (`Sim.Params` `ton` `toff` `td` `n`)"
                ),
            ),
        };
        let doc = format!(
            "The stimulus of the symbol's KiCad simulation as a sequence for \
             [`new`](Self::new): {doc}."
        );
        quote! {
            #[doc = #doc]
            pub fn kicad_sequence() -> #krate::spice::digital::DigitalSequence {
                #body
            }
        }
    });
    let file_items = file_items(&def.part, krate);

    IbisItems {
        fields: quote!({ inner: #ibis::#inner }),
        items: quote! {
            #file_items
            /// The pin of the component in the IBIS file.
            pub const IBIS_PIN: &'static str = #pin;

            #[doc = #new_doc]
            ///
            /// # Errors
            /// If the file cannot be read at run time, or no longer has the pins and models that
            /// were checked when compiling.
            #panics
            pub fn new(
                corner: #krate::ibis::Corner #sequence_param
            ) -> ::core::result::Result<Self, #ibis::IbisBufferError> {
                #warn
                let buffer = #ibis::#buffer::#call?;
                ::core::result::Result::Ok(#name {
                    inner: #ibis::#inner::new(buffer #sequence_arg),
                })
            }

            /// The buffer (or buffers of the pair) of the model.
            pub fn buffer(&self) -> &#ibis::#buffer {
                self.inner.buffer()
            }

            #sequence_fn
            #kicad_sequence
        },
        spice_fn: quote! {
            fn spice(&self) -> ::core::option::Option<#krate::spice::SpicePartInfo> {
                #ibis::__kicad::renamed_spice(
                    &self.inner,
                    &[#((#ibis::#inner::#inner_pins, #nodes)),*],
                ) #unused
            }
        },
    }
}
