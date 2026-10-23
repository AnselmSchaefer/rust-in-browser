//! `wasm-bindgen-wasi INPUT.wasm OUT_DIR [NAME]`: writes NAME.js and NAME_bg.wasm for `--target web`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, out_dir, rest @ ..] = args.as_slice() else {
        eprintln!("usage: wasm-bindgen-wasi INPUT.wasm OUT_DIR [NAME]");
        return ExitCode::from(2);
    };
    let mut bindgen = wasm_bindgen_cli_support::Bindgen::new();
    bindgen.input_path(input);
    if let Some(name) = rest.first() {
        bindgen.out_name(name);
    }
    let generated = bindgen.web(true).and_then(|b| b.typescript(false).generate(out_dir));
    match generated {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:?}");
            ExitCode::FAILURE
        }
    }
}
