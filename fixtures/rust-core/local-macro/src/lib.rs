use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn tag(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    item
}
