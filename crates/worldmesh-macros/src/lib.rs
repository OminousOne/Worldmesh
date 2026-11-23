//! `#[derive(Component)]` and `#[derive(Message)]` for Worldmesh games. Use them through the
//! `worldmesh` crate.
//!
//! The name a component or message is known by (on the wire, and in clients) defaults to the
//! type's name in snake_case (`UseAbility` is `use_ability`). Pick another with
//! `#[worldmesh(name = "combat.use")]`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, LitStr};

/// Implement `worldmesh::Component`.
#[proc_macro_derive(Component, attributes(worldmesh))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    derive(input, quote!(::worldmesh::Component))
}

/// Implement `worldmesh::Message` (for commands, actions and events).
#[proc_macro_derive(Message, attributes(worldmesh))]
pub fn derive_message(input: TokenStream) -> TokenStream {
    derive(input, quote!(::worldmesh::Message))
}

fn derive(input: TokenStream, trait_path: proc_macro2::TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = match name_attr(&input) {
        Ok(Some(name)) => name,
        Ok(None) => snake_case(&input.ident.to_string()),
        Err(e) => return e.to_compile_error().into(),
    };
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    quote! {
        impl #impl_generics #trait_path for #ident #ty_generics #where_clause {
            const NAME: &'static str = #name;
        }
    }
    .into()
}

/// The `name` in `#[worldmesh(name = "...")]`, if given.
fn name_attr(input: &DeriveInput) -> syn::Result<Option<String>> {
    let mut name = None;
    for attr in input
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("worldmesh"))
    {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                name = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("expected `name = \"...\"`"))
            }
        })?;
    }
    Ok(name)
}

fn snake_case(ident: &str) -> String {
    let mut out = String::new();
    for (i, c) in ident.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}
