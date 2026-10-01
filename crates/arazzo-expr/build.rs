use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    const ABNF: &str = "grammar/arazzo-1.1-runtime-expression.abnf";
    const DECISIONS: &str = "grammar/arazzo.pest";
    println!("cargo:rerun-if-changed={ABNF}");
    println!("cargo:rerun-if-changed={DECISIONS}");
    let source = fs::read_to_string(ABNF)?;
    // The published HTML block has a uniform two-space presentation margin.
    // Remove only that margin for the converter; retain continuation indentation.
    let conversion_source: String = source
        .lines()
        .map(|line| format!("{}\n", line.strip_prefix("  ").unwrap_or(line)))
        .collect();
    let rules = abnf_to_pest::parse_abnf(&conversion_source)
        .map_err(|error| format!("could not parse {ABNF}: {error:?}"))?;
    let grammar = format!(
        "// Generated from {ABNF}; see its provenance header.\n{}\n{}",
        abnf_to_pest::render_rules_to_pest(rules).pretty(100),
        fs::read_to_string(DECISIONS)?
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    fs::write(output.join("generated.pest"), &grammar)?;
    fs::write(
        output.join("runtime_expression_parser.rs"),
        format!(
            "// Generated from {ABNF} and {DECISIONS}.\n\
             #[derive(pest_derive::Parser)]\n\
             #[grammar_inline = {grammar:?}]\n\
             pub(crate) struct RuntimeExpressionParser;\n"
        ),
    )?;
    Ok(())
}
