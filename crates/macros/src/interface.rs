//! `#[derive(Interface)]`: the interface of a subcircuit as a struct generic over its fields.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, GenericParam, Ident, Type, spanned::Spanned};

pub fn derive(input: &DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => return Err(not_supported(input)),
        },
        _ => return Err(not_supported(input)),
    };
    let param = type_param(input)?;
    let mut idents = Vec::new();
    for field in fields {
        let is_param =
            matches!(&field.ty, Type::Path(ty) if ty.qself.is_none() && ty.path.is_ident(param));
        if !is_param {
            return Err(syn::Error::new(
                field.ty.span(),
                format!("every field of an `Interface` must have the type `{param}`"),
            ));
        }
        idents.push(field.ident.as_ref().expect("named field"));
    }
    let indices = 0..idents.len();
    let names = idents.iter().map(|ident| ident.to_string());
    Ok(quote! {
        impl<#param> ::cuprite_wire::subcircuit::Interface<#param> for #name<#param> {
            type With<__CupriteValue> = #name<__CupriteValue>;

            fn from_fn(mut f: impl FnMut(usize) -> #param) -> Self {
                Self { #( #idents: f(#indices), )* }
            }

            fn values(&self) -> ::std::vec::Vec<&#param> {
                ::std::vec![ #( &self.#idents, )* ]
            }

            fn names() -> ::std::vec::Vec<::std::string::String> {
                ::std::vec![ #( ::std::string::String::from(#names), )* ]
            }
        }
    })
}

/// The one type parameter, without bounds (a default such as `T = Node` is allowed).
fn type_param(input: &DeriveInput) -> syn::Result<&Ident> {
    let generics = &input.generics;
    let mut params = generics.params.iter();
    match (params.next(), params.next()) {
        (Some(GenericParam::Type(param)), None)
            if param.bounds.is_empty() && generics.where_clause.is_none() =>
        {
            Ok(&param.ident)
        }
        _ => Err(syn::Error::new(
            if generics.params.is_empty() {
                input.ident.span()
            } else {
                generics.params.span()
            },
            "an `Interface` must have exactly one type parameter without bounds, \
             e.g. `struct Ports<T = Node> { input: T, output: T }`",
        )),
    }
}

fn not_supported(input: &DeriveInput) -> syn::Error {
    syn::Error::new(
        input.ident.span(),
        "`Interface` can only be derived for a struct with named fields",
    )
}
