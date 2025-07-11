use std::collections::HashMap;

use ark_ff::PrimeField;
use ark_r1cs_std::{fields::fp::FpVar, R1CSVar};
use folding_schemes::Error;
use noname::{
    backends::{r1cs::R1CS, BackendField},
    circuit_writer::CircuitWriter,
    compiler::{typecheck_next_file, Sources},
    inputs::JsonInputs,
    type_checker::TypeChecker,
    witness::CompiledCircuit,
};
use serde_json::json;

pub struct NonameInputs(pub JsonInputs);

impl<F: PrimeField> From<(&Vec<F>, String)> for NonameInputs {
    fn from(value: (&Vec<F>, String)) -> Self {
        let (values, key) = value;
        let mut inputs = HashMap::new();
        if values.is_empty() {
            NonameInputs(JsonInputs(inputs))
        } else {
            let field_elements: Vec<String> = values
                .iter()
                .map(|value| {
                    if value.is_zero() {
                        "0".to_string()
                    } else {
                        value.to_string()
                    }
                })
                .collect();
            inputs.insert(key, json!(field_elements));
            NonameInputs(JsonInputs(inputs))
        }
    }
}

impl NonameInputs {
    pub fn from_fpvars<F: PrimeField>(value: (&Vec<FpVar<F>>, String)) -> Self {
        let (values, key) = value;
        let mut inputs = HashMap::new();
        if !values.is_empty() {
            let field_elements: Vec<String> = values
                .iter()
                .map(|var| var.value().unwrap_or_default().to_string())
                .collect::<Vec<String>>();
            inputs.insert(key, json!(field_elements));
        }
        NonameInputs(JsonInputs(inputs))
    }
}

// from: https://github.com/zksecurity/noname/blob/main/src/tests/modules.rs
// TODO: this will not work in the case where we are using libraries
pub fn compile_source_code<BF: BackendField>(
    code: &str,
) -> Result<CompiledCircuit<R1CS<BF>>, Error> {
    let mut sources = Sources::new();

    // parse the transitive dependency
    let mut checker = TypeChecker::<R1CS<BF>>::new();
    let _ = typecheck_next_file(
        &mut checker,
        None,
        &mut sources,
        "main.no".to_string(),
        code.to_string(),
        0,
    )
    .unwrap();
    let r1cs = R1CS::<BF>::new();
    // compile
    CircuitWriter::generate_circuit(checker, r1cs).map_err(|_| {
        Error::Other("Encountered an error while compiling a noname circuit".to_owned())
    })
}
