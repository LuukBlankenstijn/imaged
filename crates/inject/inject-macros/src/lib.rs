use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, Token, parse_macro_input, parse_quote, punctuated::Punctuated};

#[proc_macro_attribute]
pub fn inject(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut func = parse_macro_input!(item as ItemFn);
    let dependencies = parse_macro_input!(attr with Punctuated<FnArg, Token![,]>::parse_terminated);

    let bindings = dependencies.iter().map(|dependency| match dependency {
        FnArg::Typed(pat_type) => {
            let name = &pat_type.pat;
            let ty = &pat_type.ty;
            quote! { let #name = <#ty as ::inject::Resolve>::resolve(); }
        }
        FnArg::Receiver(receiver) => {
            syn::Error::new_spanned(receiver, "expected `name: Type`").to_compile_error()
        }
    });

    let statements = &func.block.stmts;
    func.block = parse_quote!({ #(#bindings)* #(#statements)* });

    quote!(#func).into()
}
