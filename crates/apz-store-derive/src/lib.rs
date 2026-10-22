//! `derive(Table)` and `derive(Column)` for `apz-store` (see `apz_store::table` for what they generate).
//!
//! - `#[derive(Table)]` on a struct with named fields: one table, typed columns, child tables for collections.
//! - `#[derive(Table)]` on an enum with data: stored flattened (a `kind` column and nullable columns per variant
//!   field), or as a table of those columns when it is a child.
//! - `#[derive(Column)]` on an enum of unit variants (text with a `CHECK` over the variant names) or on a newtype
//!   struct (the inner type's column).
//!
//! Variant and table names follow `#[table(rename = …)]`, then serde's `rename`/`rename_all`, then snake case.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as Tokens};
use quote::quote;
use syn::meta::ParseNestedMeta;
use syn::{Attribute, Data, DeriveInput, Fields, LitStr, Type, parse_macro_input};

#[proc_macro_derive(Table, attributes(table))]
pub fn derive_table(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let out = match &input.data {
        Data::Struct(_) => table_struct(&input),
        Data::Enum(_) => table_enum(&input),
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "Table cannot be derived for a union",
        )),
    };
    out.unwrap_or_else(syn::Error::into_compile_error).into()
}

#[proc_macro_derive(Column, attributes(column, table))]
pub fn derive_column(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    column(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// How a field is stored.
#[derive(Clone, PartialEq)]
enum Kind {
    Column,
    Flatten,
    Child(Option<String>),
    List(Option<String>),
    Set(Option<String>),
    Map(Option<String>),
    MapValue(Option<String>),
    Content,
    Skip,
}

#[derive(Default)]
struct FieldAttrs {
    key: bool,
    kind: Option<Kind>,
    rename: Option<String>,
    index: bool,
    unique: bool,
}

fn optional_name(meta: &ParseNestedMeta) -> syn::Result<Option<String>> {
    if meta.input.peek(syn::Token![=]) {
        let lit: LitStr = meta.value()?.parse()?;
        Ok(Some(lit.value()))
    } else {
        Ok(None)
    }
}

fn field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("table")) {
        attr.parse_nested_meta(|meta| {
            let set_kind = |out: &mut FieldAttrs, kind: Kind| -> syn::Result<()> {
                if out.kind.is_some() {
                    return Err(meta.error("a field has one storage kind"));
                }
                out.kind = Some(kind);
                Ok(())
            };
            if meta.path.is_ident("key") {
                out.key = true;
            } else if meta.path.is_ident("skip") {
                set_kind(&mut out, Kind::Skip)?;
            } else if meta.path.is_ident("flatten") {
                set_kind(&mut out, Kind::Flatten)?;
            } else if meta.path.is_ident("content") {
                set_kind(&mut out, Kind::Content)?;
            } else if meta.path.is_ident("child") {
                let name = optional_name(&meta)?;
                set_kind(&mut out, Kind::Child(name))?;
            } else if meta.path.is_ident("list") {
                let name = optional_name(&meta)?;
                set_kind(&mut out, Kind::List(name))?;
            } else if meta.path.is_ident("set") {
                let name = optional_name(&meta)?;
                set_kind(&mut out, Kind::Set(name))?;
            } else if meta.path.is_ident("map") {
                let name = optional_name(&meta)?;
                set_kind(&mut out, Kind::Map(name))?;
            } else if meta.path.is_ident("map_value") {
                let name = optional_name(&meta)?;
                set_kind(&mut out, Kind::MapValue(name))?;
            } else if meta.path.is_ident("rename") {
                let lit: LitStr = meta.value()?.parse()?;
                out.rename = Some(lit.value());
            } else if meta.path.is_ident("index") {
                out.index = true;
            } else if meta.path.is_ident("unique") {
                out.unique = true;
            } else {
                return Err(meta.error(
                    "unknown table attribute (key, skip, flatten, content, child, list, set, map, map_value, rename, index, unique)",
                ));
            }
            Ok(())
        })?;
    }
    if out.key && !matches!(out.kind, None | Some(Kind::Column)) {
        return Err(syn::Error::new(
            Span::call_site(),
            "a key field must be a plain column",
        ));
    }
    Ok(out)
}

/// The type's own options: `#[table(name = …)]`, and serde's `rename_all` for variant names.
struct TypeAttrs {
    name: Option<String>,
    rename_all: Option<String>,
}

/// Skips the value of a nested meta item we do not read (serde's other attributes).
fn skip_meta(meta: &ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        let _: syn::Expr = meta.value()?.parse()?;
    } else if meta.input.peek(syn::token::Paren) {
        meta.parse_nested_meta(|inner| skip_meta(&inner))?;
    }
    Ok(())
}

fn type_attrs(attrs: &[Attribute]) -> syn::Result<TypeAttrs> {
    let mut out = TypeAttrs {
        name: None,
        rename_all: None,
    };
    for attr in attrs {
        if attr.path().is_ident("table") || attr.path().is_ident("column") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let lit: LitStr = meta.value()?.parse()?;
                    out.name = Some(lit.value());
                } else if meta.path.is_ident("rename_all") {
                    let lit: LitStr = meta.value()?.parse()?;
                    out.rename_all = Some(lit.value());
                } else {
                    return Err(meta.error("unknown attribute (name, rename_all)"));
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("serde") {
            // serde's attributes are read only for rename_all; the rest is skipped.
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") && meta.input.peek(syn::Token![=]) {
                    let lit: LitStr = meta.value()?.parse()?;
                    if out.rename_all.is_none() {
                        out.rename_all = Some(lit.value());
                    }
                    Ok(())
                } else {
                    skip_meta(&meta)
                }
            });
        }
    }
    Ok(out)
}

/// A variant's own rename: `#[table(rename = …)]`, then `#[serde(rename = …)]`.
fn variant_rename(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut ours = None;
    let mut serde = None;
    for attr in attrs {
        if attr.path().is_ident("table") || attr.path().is_ident("column") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let lit: LitStr = meta.value()?.parse()?;
                    ours = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("unknown variant attribute (rename)"))
                }
            })?;
        } else if attr.path().is_ident("serde") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") && meta.input.peek(syn::Token![=]) {
                    let lit: LitStr = meta.value()?.parse()?;
                    serde = Some(lit.value());
                    Ok(())
                } else {
                    skip_meta(&meta)
                }
            });
        }
    }
    Ok(ours.or(serde))
}

/// `SomeName` → `some_name`.
fn snake(ident: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = ident.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower =
                i > 0 && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if i > 0 && (prev_lower || (next_lower && chars[i - 1].is_uppercase())) {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(*c);
        }
    }
    out
}

/// A variant's name under serde's `rename_all` rule.
fn renamed(ident: &str, rule: Option<&str>) -> String {
    let snake = snake(ident);
    match rule {
        Some("lowercase") => ident.to_lowercase(),
        Some("UPPERCASE") => ident.to_uppercase(),
        Some("PascalCase") => ident.to_string(),
        Some("camelCase") => {
            let mut c = ident.chars();
            c.next()
                .map(|f| f.to_lowercase().chain(c).collect())
                .unwrap_or_default()
        }
        Some("SCREAMING_SNAKE_CASE") => snake.to_uppercase(),
        Some("kebab-case") => snake.replace('_', "-"),
        Some("SCREAMING-KEBAB-CASE") => snake.replace('_', "-").to_uppercase(),
        _ => snake,
    }
}

/// Whether a type's last path segment is `name` (`Option<T>`, `std::option::Option<T>`).
fn last_segment_is(ty: &Type, name: &str) -> bool {
    match ty {
        Type::Path(p) => p.path.segments.last().is_some_and(|s| s.ident == name),
        _ => false,
    }
}

fn table_struct(input: &DeriveInput) -> syn::Result<Tokens> {
    let Data::Struct(data) = &input.data else {
        unreachable!()
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "Table needs a struct with named fields",
        ));
    };
    let ident = &input.ident;
    let (impl_g, ty_g, where_c) = input.generics.split_for_impl();
    let attrs = type_attrs(&input.attrs)?;
    let name = attrs.name.unwrap_or_else(|| snake(&ident.to_string()));

    let mut own_defs = Vec::new();
    let mut col_defs = Vec::new();
    let mut child_defs = Vec::new();
    let mut index_defs = Vec::new();
    let mut own_puts = Vec::new();
    let mut col_puts = Vec::new();
    let mut child_puts = Vec::new();
    let mut own_takes = Vec::new();
    let mut col_takes = Vec::new();
    let mut child_takes = Vec::new();
    let mut flat_defs = Vec::new();
    let mut flat_puts = Vec::new();
    let mut flat_takes = Vec::new();
    let mut field_names = Vec::new();
    let mut has_children = false;
    let mut own_count = 0usize;
    let mut child_index = 0usize;

    for field in &fields.named {
        let fident = field.ident.as_ref().expect("named");
        let fty = &field.ty;
        let fa = field_attrs(&field.attrs)?;
        let col = fa
            .rename
            .clone()
            .unwrap_or_else(|| fident.to_string().trim_start_matches("r#").to_string());
        let fname = fident.to_string();
        field_names.push(fident.clone());
        let kind = fa.kind.clone().unwrap_or(Kind::Column);
        let field_err =
            quote! { |e| ::apz_store::StoreError::invalid(format!("{}.{}: {}", #name, #fname, e)) };
        match kind.clone() {
            Kind::Skip => {
                child_takes
                    .push(quote! { let #fident: #fty = ::core::default::Default::default(); });
                flat_takes
                    .push(quote! { let #fident: #fty = ::core::default::Default::default(); });
            }
            Kind::Column => {
                let def = quote! { __t::ColumnDef::of::<#fty>(#col) };
                let put =
                    quote! { <#fty as __t::Column>::to_value(&self.#fident).map_err(#field_err)? };
                if fa.key {
                    own_count += 1;
                    own_defs.push(quote! { __own.push(#def); });
                    own_puts.push(quote! { __own.push(#put); });
                    own_takes
                        .push(quote! { let #fident: #fty = __t::take_col(&mut __own_v, #fname)?; });
                } else {
                    col_defs.push(quote! { __cols.push(#def); });
                    col_puts.push(quote! { __cols.push(#put); });
                    col_takes.push(
                        quote! { let #fident: #fty = __t::take_col(&mut __cols_v, #fname)?; },
                    );
                }
                flat_defs.push(quote! { __out.push(__t::ColumnDef::of::<#fty>(&format!("{}{}", __prefix, #col))); });
                flat_puts.push(quote! { __out.push(#put); });
                flat_takes.push(quote! { let #fident: #fty = __t::take_col(__v, #fname)?; });
                if fa.index || fa.unique {
                    let unique = fa.unique;
                    index_defs.push(quote! {
                        __def.indexes.push(__t::IndexDef { columns: vec![#col.to_string()], unique: #unique });
                    });
                }
            }
            Kind::Flatten => {
                let prefix = format!("{col}_");
                col_defs.push(quote! { <#fty as __t::Table>::flat_columns(#prefix, &mut __cols); });
                col_puts.push(quote! { <#fty as __t::Table>::flat_put(&self.#fident, &mut __cols).map_err(#field_err)?; });
                col_takes.push(
                    quote! { let #fident: #fty = <#fty as __t::Table>::flat_take(&mut __cols_v)?; },
                );
                flat_defs.push(quote! { <#fty as __t::Table>::flat_columns(&format!("{}{}", __prefix, #prefix), __out); });
                flat_puts.push(quote! { <#fty as __t::Table>::flat_put(&self.#fident, __out)?; });
                flat_takes
                    .push(quote! { let #fident: #fty = <#fty as __t::Table>::flat_take(__v)?; });
            }
            Kind::Content => {
                col_defs.push(quote! { __cols.push(__t::content_column(#col)); });
                col_puts.push(
                    quote! { __t::put_content(&self.#fident, &mut __cols).map_err(#field_err)?; },
                );
                col_takes.push(quote! { let #fident: #fty = __t::take_content(&mut __cols_v)?; });
                flat_defs.push(
                    quote! { __out.push(__t::content_column(&format!("{}{}", __prefix, #col))); },
                );
                flat_puts.push(quote! { __t::put_content(&self.#fident, __out)?; });
                flat_takes.push(quote! { let #fident: #fty = __t::take_content(__v)?; });
            }
            Kind::Child(table)
            | Kind::List(table)
            | Kind::Set(table)
            | Kind::Map(table)
            | Kind::MapValue(table) => {
                has_children = true;
                let i = child_index;
                child_index += 1;
                let table = match table {
                    Some(t) => quote! { Some(#t) },
                    None => quote! { None },
                };
                let (def, put, take) = match kind {
                    Kind::Child(_) if last_segment_is(fty, "Option") => (
                        quote! { __t::option_def::<<#fty as __t::Elem>::Item>(&__def, #col, #table) },
                        quote! { __t::put_option(&__def.children[#i], &__k, self.#fident.as_ref(), __out)?; },
                        quote! { let #fident: #fty = __t::take_option(&__def.children[#i], &__row.key, __loaded)?; },
                    ),
                    Kind::Child(_) => (
                        quote! { __t::children_def::<<#fty as __t::Elem>::Item>(&__def, #col, #table) },
                        quote! { __t::put_children(&__def.children[#i], &__k, self.#fident.iter(), __out)?; },
                        quote! { let #fident: #fty = __t::take_children(&__def.children[#i], &__row.key, __loaded)?.into_iter().collect(); },
                    ),
                    Kind::List(_) => (
                        quote! { __t::list_def::<<#fty as __t::Elem>::Item>(&__def, #col, #table) },
                        quote! { __t::put_list(&__def.children[#i], &__k, self.#fident.iter(), __out)?; },
                        quote! { let #fident: #fty = __t::take_list(&__def.children[#i], &__row.key, __loaded)?.into_iter().collect(); },
                    ),
                    Kind::Set(_) => (
                        quote! { __t::set_def::<<#fty as __t::Elem>::Item>(&__def, #col, #table) },
                        quote! { __t::put_set(&__def.children[#i], &__k, self.#fident.iter(), __out)?; },
                        quote! { let #fident: #fty = __t::take_set(&__def.children[#i], &__row.key, __loaded)?.into_iter().collect(); },
                    ),
                    Kind::Map(_) => (
                        quote! { __t::map_def::<<#fty as __t::MapElem>::Key, <#fty as __t::MapElem>::Value>(&__def, #col, #table) },
                        quote! { __t::put_map(&__def.children[#i], &__k, self.#fident.iter(), __out)?; },
                        quote! { let #fident: #fty = __t::take_map(&__def.children[#i], &__row.key, __loaded)?.into_iter().collect(); },
                    ),
                    Kind::MapValue(_) => (
                        quote! { __t::map_value_def::<<#fty as __t::MapElem>::Key, <#fty as __t::MapElem>::Value>(&__def, #col, #table) },
                        quote! { __t::put_map_values(&__def.children[#i], &__k, self.#fident.iter(), __out)?; },
                        quote! { let #fident: #fty = __t::take_map_values(&__def.children[#i], &__row.key, __loaded)?.into_iter().collect(); },
                    ),
                    _ => unreachable!(),
                };
                child_defs.push(quote! { let __c = #def; __def.children.push(__c); });
                child_puts.push(put);
                child_takes.push(take);
            }
        }
    }

    let flat_body = if has_children {
        let msg = format!("{name} has child tables and cannot be flattened");
        (
            quote! { panic!(#msg) },
            quote! { Err(::apz_store::StoreError::invalid(#msg)) },
            quote! { Err(::apz_store::StoreError::invalid(#msg)) },
        )
    } else {
        (
            quote! { #(#flat_defs)* },
            quote! { #(#flat_puts)* Ok(()) },
            quote! { #(#flat_takes)* Ok(Self { #(#field_names),* }) },
        )
    };
    let (flat_cols_body, flat_put_body, flat_take_body) = flat_body;

    Ok(quote! {
        #[automatically_derived]
        impl #impl_g ::apz_store::Table for #ident #ty_g #where_c {
            const NAME: &'static str = #name;

            fn define(ctx: ::apz_store::table::Ctx) -> ::apz_store::TableDef {
                use ::apz_store::table as __t;
                let mut __own: ::std::vec::Vec<__t::ColumnDef> = ::std::vec::Vec::new();
                #(#own_defs)*
                let mut __cols: ::std::vec::Vec<__t::ColumnDef> = ::std::vec::Vec::new();
                #(#col_defs)*
                let mut __def = __t::struct_table(&ctx, __own, __cols);
                #(#child_defs)*
                #(#index_defs)*
                __def
            }

            fn put(
                &self,
                __def: &::apz_store::TableDef,
                __key: ::std::vec::Vec<::apz_store::table::Value>,
                __out: &mut ::apz_store::RowSet,
            ) -> ::std::result::Result<(), ::apz_store::StoreError> {
                use ::apz_store::table as __t;
                let mut __own: ::std::vec::Vec<__t::Value> = ::std::vec::Vec::new();
                #(#own_puts)*
                let mut __cols: ::std::vec::Vec<__t::Value> = ::std::vec::Vec::new();
                #(#col_puts)*
                let (__k, __lead) = __t::layout(__def, __key, __own);
                let __k = __t::put_row(__def, __k, __lead, __cols, __out);
                let _ = &__k;
                #(#child_puts)*
                Ok(())
            }

            fn take(
                __def: &::apz_store::TableDef,
                __row: &::apz_store::Row,
                __loaded: &::apz_store::Loaded,
            ) -> ::std::result::Result<Self, ::apz_store::StoreError> {
                use ::apz_store::table as __t;
                let (mut __own_v, mut __cols_v) = __t::split(__def, __row, #own_count);
                let _ = (&mut __own_v, &mut __cols_v, __loaded);
                #(#own_takes)*
                #(#col_takes)*
                #(#child_takes)*
                Ok(Self { #(#field_names),* })
            }

            fn flat_columns(__prefix: &str, __out: &mut ::std::vec::Vec<::apz_store::ColumnDef>) {
                use ::apz_store::table as __t;
                let _ = (__prefix, &__out);
                #flat_cols_body
            }

            fn flat_put(
                &self,
                __out: &mut ::std::vec::Vec<::apz_store::table::Value>,
            ) -> ::std::result::Result<(), ::apz_store::StoreError> {
                use ::apz_store::table as __t;
                let _ = &__out;
                #flat_put_body
            }

            fn flat_take(
                __v: &mut ::std::vec::IntoIter<::apz_store::table::Value>,
            ) -> ::std::result::Result<Self, ::apz_store::StoreError> {
                use ::apz_store::table as __t;
                let _ = &__v;
                #flat_take_body
            }
        }
    })
}

/// One stored field of an enum variant: its column name, binding and type.
struct VariantField {
    col: String,
    bind: syn::Ident,
    ty: Type,
    content: bool,
    /// A sub-struct stored as nullable columns prefixed `<variant>_<field>_`.
    flatten: bool,
}

fn table_enum(input: &DeriveInput) -> syn::Result<Tokens> {
    let Data::Enum(data) = &input.data else {
        unreachable!()
    };
    let ident = &input.ident;
    let (impl_g, ty_g, where_c) = input.generics.split_for_impl();
    let attrs = type_attrs(&input.attrs)?;
    let name = attrs
        .name
        .clone()
        .unwrap_or_else(|| snake(&ident.to_string()));

    let mut kinds = Vec::new();
    let mut col_defs = Vec::new();
    let mut put_arms = Vec::new();
    let mut take_arms = Vec::new();
    // Every variant's columns in order: (variant index, field).
    let mut all: Vec<(usize, VariantField)> = Vec::new();
    for (vi, variant) in data.variants.iter().enumerate() {
        let kind = variant_rename(&variant.attrs)?
            .unwrap_or_else(|| renamed(&variant.ident.to_string(), attrs.rename_all.as_deref()));
        kinds.push(kind.clone());
        let vsnake = snake(&variant.ident.to_string());
        let fields: Vec<VariantField> = match &variant.fields {
            Fields::Unit => Vec::new(),
            Fields::Named(named) => named
                .named
                .iter()
                .map(|f| {
                    let fa = field_attrs(&f.attrs)?;
                    let fid = f.ident.clone().expect("named");
                    let col = format!(
                        "{vsnake}_{}",
                        fa.rename.clone().unwrap_or_else(|| fid.to_string())
                    );
                    if !matches!(
                        fa.kind,
                        None | Some(Kind::Column) | Some(Kind::Content) | Some(Kind::Flatten)
                    ) {
                        return Err(syn::Error::new_spanned(
                            f,
                            "an enum variant's fields are columns, content or flatten",
                        ));
                    }
                    Ok(VariantField {
                        col,
                        bind: fid,
                        ty: f.ty.clone(),
                        content: fa.kind == Some(Kind::Content),
                        flatten: fa.kind == Some(Kind::Flatten),
                    })
                })
                .collect::<syn::Result<_>>()?,
            Fields::Unnamed(unnamed) => {
                let n = unnamed.unnamed.len();
                unnamed
                    .unnamed
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let fa = field_attrs(&f.attrs)?;
                        let col = if n == 1 {
                            vsnake.clone()
                        } else {
                            format!("{vsnake}_{i}")
                        };
                        let bind = syn::Ident::new(&format!("__f{i}"), Span::call_site());
                        Ok(VariantField {
                            col,
                            bind,
                            ty: f.ty.clone(),
                            content: fa.kind == Some(Kind::Content),
                            flatten: fa.kind == Some(Kind::Flatten),
                        })
                    })
                    .collect::<syn::Result<_>>()?
            }
        };
        for f in fields {
            all.push((vi, f));
        }
    }

    let kind_list = kinds
        .iter()
        .map(|k| format!("'{}'", k.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    col_defs.push(quote! {
        __out.push(__t::ColumnDef {
            name: format!("{}kind", __prefix),
            ty: __t::SqlType::Text,
            nullable: false,
            check: Some(format!("{} IN ({})", __t::quote(&format!("{}kind", __prefix)), #kind_list)),
        });
    });
    for (_, f) in &all {
        let col = &f.col;
        let ty = &f.ty;
        if f.flatten {
            col_defs.push(quote! {
                let mut __inner = ::std::vec::Vec::new();
                <#ty as __t::Table>::flat_columns(&format!("{}{}_", __prefix, #col), &mut __inner);
                for mut __c in __inner {
                    __c.nullable = true;
                    __out.push(__c);
                }
            });
        } else if f.content {
            col_defs.push(
                quote! { __out.push(__t::content_column(&format!("{}{}", __prefix, #col))); },
            );
        } else {
            col_defs.push(quote! {
                let mut __c = __t::ColumnDef::of::<#ty>(&format!("{}{}", __prefix, #col));
                __c.nullable = true;
                __out.push(__c);
            });
        }
    }

    for (vi, variant) in data.variants.iter().enumerate() {
        let vident = &variant.ident;
        let kind = &kinds[vi];
        let mine: Vec<&VariantField> = all
            .iter()
            .filter(|(i, _)| *i == vi)
            .map(|(_, f)| f)
            .collect();
        let binds: Vec<&syn::Ident> = mine.iter().map(|f| &f.bind).collect();
        let pattern = match &variant.fields {
            Fields::Unit => quote! { #ident::#vident },
            Fields::Named(_) => quote! { #ident::#vident { #(#binds),* } },
            Fields::Unnamed(_) => quote! { #ident::#vident ( #(#binds),* ) },
        };
        let mut pushes = Vec::new();
        for (i, f) in &all {
            let bind = &f.bind;
            let ty = &f.ty;
            if *i == vi {
                if f.flatten {
                    pushes.push(quote! { <#ty as __t::Table>::flat_put(#bind, __out)?; });
                } else if f.content {
                    pushes.push(quote! { __t::put_content(#bind, __out)?; });
                } else {
                    pushes.push(quote! { __out.push(<#ty as __t::Column>::to_value(#bind)?); });
                }
            } else if f.flatten {
                pushes.push(quote! {
                    let mut __inner = ::std::vec::Vec::new();
                    <#ty as __t::Table>::flat_columns("", &mut __inner);
                    __out.extend(::std::iter::repeat_n(__t::Value::Null, __inner.len()));
                });
            } else {
                pushes.push(quote! { __out.push(__t::Value::Null); });
            }
        }
        put_arms.push(quote! {
            #pattern => {
                __out.push(__t::Value::Text(#kind.to_string()));
                #(#pushes)*
            }
        });
        let mut takes = Vec::new();
        for (i, f) in &all {
            let bind = &f.bind;
            let ty = &f.ty;
            let col = &f.col;
            if *i == vi {
                if f.flatten {
                    takes.push(quote! { let #bind: #ty = <#ty as __t::Table>::flat_take(__v)?; });
                } else if f.content {
                    takes.push(quote! { let #bind: #ty = __t::take_content(__v)?; });
                } else {
                    takes.push(quote! { let #bind: #ty = __t::take_col(__v, #col)?; });
                }
            } else if f.flatten {
                takes.push(quote! {
                    let mut __inner = ::std::vec::Vec::new();
                    <#ty as __t::Table>::flat_columns("", &mut __inner);
                    for _ in 0..__inner.len() {
                        __t::next(__v)?;
                    }
                });
            } else {
                takes.push(quote! { __t::next(__v)?; });
            }
        }
        take_arms.push(quote! {
            #kind => {
                #(#takes)*
                Ok(#pattern)
            }
        });
    }

    Ok(quote! {
        #[automatically_derived]
        impl #impl_g ::apz_store::Table for #ident #ty_g #where_c {
            const NAME: &'static str = #name;

            fn define(ctx: ::apz_store::table::Ctx) -> ::apz_store::TableDef {
                let mut __cols = ::std::vec::Vec::new();
                <Self as ::apz_store::Table>::flat_columns("", &mut __cols);
                ::apz_store::table::struct_table(&ctx, ::std::vec::Vec::new(), __cols)
            }

            fn put(
                &self,
                __def: &::apz_store::TableDef,
                __key: ::std::vec::Vec<::apz_store::table::Value>,
                __out: &mut ::apz_store::RowSet,
            ) -> ::std::result::Result<(), ::apz_store::StoreError> {
                let (__k, __lead) = ::apz_store::table::layout(__def, __key, ::std::vec::Vec::new());
                let mut __cols = ::std::vec::Vec::new();
                <Self as ::apz_store::Table>::flat_put(self, &mut __cols)?;
                ::apz_store::table::put_row(__def, __k, __lead, __cols, __out);
                Ok(())
            }

            fn take(
                __def: &::apz_store::TableDef,
                __row: &::apz_store::Row,
                _loaded: &::apz_store::Loaded,
            ) -> ::std::result::Result<Self, ::apz_store::StoreError> {
                let (_, mut __cols) = ::apz_store::table::split(__def, __row, 0);
                <Self as ::apz_store::Table>::flat_take(&mut __cols)
            }

            fn flat_columns(__prefix: &str, __out: &mut ::std::vec::Vec<::apz_store::ColumnDef>) {
                use ::apz_store::table as __t;
                #(#col_defs)*
            }

            fn flat_put(
                &self,
                __out: &mut ::std::vec::Vec<::apz_store::table::Value>,
            ) -> ::std::result::Result<(), ::apz_store::StoreError> {
                use ::apz_store::table as __t;
                match self {
                    #(#put_arms)*
                }
                Ok(())
            }

            fn flat_take(
                __v: &mut ::std::vec::IntoIter<::apz_store::table::Value>,
            ) -> ::std::result::Result<Self, ::apz_store::StoreError> {
                use ::apz_store::table as __t;
                let __kind: String = __t::take_col(__v, "kind")?;
                match __kind.as_str() {
                    #(#take_arms)*
                    other => Err(::apz_store::StoreError::invalid(format!("{}: unknown kind {:?}", #name, other))),
                }
            }
        }
    })
}

fn column(input: &DeriveInput) -> syn::Result<Tokens> {
    let ident = &input.ident;
    let (impl_g, ty_g, where_c) = input.generics.split_for_impl();
    match &input.data {
        Data::Struct(data) => {
            let Fields::Unnamed(fields) = &data.fields else {
                return Err(syn::Error::new_spanned(
                    ident,
                    "Column on a struct needs a newtype: struct Id(Inner)",
                ));
            };
            if fields.unnamed.len() != 1 {
                return Err(syn::Error::new_spanned(
                    ident,
                    "Column on a struct needs exactly one field",
                ));
            }
            let inner = &fields.unnamed[0].ty;
            Ok(quote! {
                #[automatically_derived]
                impl #impl_g ::apz_store::Column for #ident #ty_g #where_c {
                    const TYPE: ::apz_store::SqlType = <#inner as ::apz_store::Column>::TYPE;
                    const NULLABLE: bool = <#inner as ::apz_store::Column>::NULLABLE;
                    fn check(quoted: &str) -> Option<String> {
                        <#inner as ::apz_store::Column>::check(quoted)
                    }
                    fn to_value(&self) -> ::std::result::Result<::apz_store::table::Value, ::apz_store::StoreError> {
                        <#inner as ::apz_store::Column>::to_value(&self.0)
                    }
                    fn from_value(value: ::apz_store::table::Value) -> ::std::result::Result<Self, ::apz_store::StoreError> {
                        <#inner as ::apz_store::Column>::from_value(value).map(#ident)
                    }
                }
            })
        }
        Data::Enum(data) => {
            let attrs = type_attrs(&input.attrs)?;
            let mut names = Vec::new();
            let mut to = Vec::new();
            let mut from = Vec::new();
            for variant in &data.variants {
                if !matches!(variant.fields, Fields::Unit) {
                    return Err(syn::Error::new_spanned(
                        variant,
                        "Column needs unit variants; derive Table and flatten an enum with data",
                    ));
                }
                let vident = &variant.ident;
                let name = variant_rename(&variant.attrs)?
                    .unwrap_or_else(|| renamed(&vident.to_string(), attrs.rename_all.as_deref()));
                to.push(quote! { #ident::#vident => #name });
                from.push(quote! { #name => Ok(#ident::#vident) });
                names.push(format!("'{}'", name.replace('\'', "''")));
            }
            let list = names.join(", ");
            let type_name = ident.to_string();
            Ok(quote! {
                #[automatically_derived]
                impl #impl_g ::apz_store::Column for #ident #ty_g #where_c {
                    const TYPE: ::apz_store::SqlType = ::apz_store::SqlType::Text;
                    fn check(quoted: &str) -> Option<String> {
                        Some(format!("{} IN ({})", quoted, #list))
                    }
                    fn to_value(&self) -> ::std::result::Result<::apz_store::table::Value, ::apz_store::StoreError> {
                        Ok(::apz_store::table::Value::Text(match self { #(#to),* }.to_string()))
                    }
                    fn from_value(value: ::apz_store::table::Value) -> ::std::result::Result<Self, ::apz_store::StoreError> {
                        match value {
                            ::apz_store::table::Value::Text(s) => match s.as_str() {
                                #(#from,)*
                                other => Err(::apz_store::StoreError::invalid(format!("{} has no variant {:?}", #type_name, other))),
                            },
                            other => Err(::apz_store::StoreError::invalid(format!("{} expects text, found {:?}", #type_name, other))),
                        }
                    }
                }
            })
        }
        Data::Union(_) => Err(syn::Error::new_spanned(
            ident,
            "Column cannot be derived for a union",
        )),
    }
}
