use syntect::parsing::SyntaxSet;
fn main() { let ss=SyntaxSet::load_defaults_newlines(); println!("count={}",ss.syntaxes().len()); for s in ss.syntaxes() { println!("{}\t{}",s.name,s.file_extensions.join(",")); } }
