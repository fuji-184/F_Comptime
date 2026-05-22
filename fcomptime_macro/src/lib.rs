use proc_macro::TokenStream;

#[proc_macro]
pub fn comptime_token(input: TokenStream) -> TokenStream {
    let name = input.to_string();
    let name = name.trim().trim_matches('"');
    
    let path = format!("./comptime/{}", name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("comptime file '{}' not found, run cargo test --features=comptime first", name));
    
    content.parse().unwrap_or_else(|_| panic!("failed to parse comptime file '{}'", name))
}

#[proc_macro_attribute]
pub fn comptime_type(attr: TokenStream, item: TokenStream) -> TokenStream {
    let name = attr.to_string();
    let name = name.trim().trim_matches('"');
    
    let path = format!("./comptime/{}", name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("comptime file '{}' not found", name));
    
    let item_str = item.to_string().replace("__comptime__", &content);
    item_str.parse().unwrap()
}