use crate::commitment::CommitmentScheme;
use crate::folding::nova::{CommittedInstance, Witness};
use crate::RngCore;
use ark_crypto_primitives::sponge::Absorb;
use ark_ec::{CurveGroup, Group};
use ark_ff::PrimeField;
use ark_relations::r1cs::ConstraintSystem;
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::rand::Rng;

use super::Arith;
use crate::utils::vec::{hadamard, mat_vec_mul, vec_scalar_mul, vec_sub, SparseMatrix};
use crate::Error;

#[derive(Debug, Clone, Eq, PartialEq, CanonicalSerialize, CanonicalDeserialize)]
pub struct R1CS<F: PrimeField> {
    pub l: usize, // io len
    pub A: SparseMatrix<F>,
    pub B: SparseMatrix<F>,
    pub C: SparseMatrix<F>,
}

impl<F: PrimeField> Arith<F> for R1CS<F> {
    fn eval_relation(&self, z: &[F]) -> Result<Vec<F>, Error> {
        if z.len() != self.A.n_cols {
            return Err(Error::NotSameLength(
                "z.len()".to_string(),
                z.len(),
                "number of variables in R1CS".to_string(),
                self.A.n_cols,
            ));
        }

        let Az = mat_vec_mul(&self.A, z)?;
        let Bz = mat_vec_mul(&self.B, z)?;
        let Cz = mat_vec_mul(&self.C, z)?;
        // Multiply Cz by z[0] (u) here, allowing this method to be reused for
        // both relaxed and unrelaxed R1CS.
        let uCz = vec_scalar_mul(&Cz, &z[0]);
        let AzBz = hadamard(&Az, &Bz)?;
        vec_sub(&AzBz, &uCz)
    }

    fn params_to_le_bytes(&self) -> Vec<u8> {
        [
            self.l.to_le_bytes(),
            self.A.n_rows.to_le_bytes(),
            self.A.n_cols.to_le_bytes(),
        ]
        .concat()
    }
}

impl<F: PrimeField> R1CS<F> {
    pub fn rand<R: Rng>(rng: &mut R, n_rows: usize, n_cols: usize) -> Self {
        Self {
            l: 1,
            A: SparseMatrix::rand(rng, n_rows, n_cols),
            B: SparseMatrix::rand(rng, n_rows, n_cols),
            C: SparseMatrix::rand(rng, n_rows, n_cols),
        }
    }

    /// returns a tuple containing (w, x) (witness and public inputs respectively)
    pub fn split_z(&self, z: &[F]) -> (Vec<F>, Vec<F>) {
        (z[self.l + 1..].to_vec(), z[1..self.l + 1].to_vec())
    }
}

pub trait RelaxedR1CS<F: PrimeField, W, U>: Arith<F> {
    /// returns a dummy running instance (Witness and CommittedInstance) for the current R1CS structure
    fn dummy_running_instance(&self) -> (W, U);

    /// returns a dummy incoming instance (Witness and CommittedInstance) for the current R1CS structure
    fn dummy_incoming_instance(&self) -> (W, U);

    /// checks if the given instance is relaxed
    fn is_relaxed(w: &W, u: &U) -> bool;

    /// extracts `z`, the vector of variables, from the given Witness and CommittedInstance
    fn extract_z(w: &W, u: &U) -> Vec<F>;

    /// checks if the computed error terms correspond to the actual one in `w`
    /// or `u`
    fn check_error_terms(w: &W, u: &U, e: Vec<F>) -> Result<(), Error>;

    /// checks the tight (unrelaxed) R1CS relation
    fn check_tight_relation(&self, w: &W, u: &U) -> Result<(), Error> {
        if Self::is_relaxed(w, u) {
            return Err(Error::R1CSUnrelaxedFail);
        }

        let z = Self::extract_z(w, u);
        self.check_relation(&z)
    }

    /// checks the relaxed R1CS relation
    fn check_relaxed_relation(&self, w: &W, u: &U) -> Result<(), Error> {
        let z = Self::extract_z(w, u);
        let e = self.eval_relation(&z)?;
        Self::check_error_terms(w, u, e)
    }

    // Computes the E term, given A, B, C, z, u
    fn compute_E(
        A: &SparseMatrix<F>,
        B: &SparseMatrix<F>,
        C: &SparseMatrix<F>,
        z: &[F],
        u: &F,
    ) -> Result<Vec<F>, Error> {
        let Az = mat_vec_mul(A, z)?;
        let Bz = mat_vec_mul(B, z)?;
        let AzBz = hadamard(&Az, &Bz)?;

        let Cz = mat_vec_mul(C, z)?;
        let uCz = vec_scalar_mul(&Cz, u);
        vec_sub(&AzBz, &uCz)
    }

    pub fn check_sampled_relaxed_r1cs(&self, u: F, E: &[F], z: &[F]) -> bool {
        let sampled = RelaxedR1CS {
            l: self.l,
            A: self.A.clone(),
            B: self.B.clone(),
            C: self.C.clone(),
            u,
            E: E.to_vec(),
        };
        sampled.check_relation(z).is_ok()
    }

    // Implements sampling a (committed) RelaxedR1CS
    // See construction 5 in https://eprint.iacr.org/2023/573.pdf
    pub fn sample<C, CS>(
        &self,
        params: &CS::ProverParams,
        mut rng: impl RngCore,
    ) -> Result<(CommittedInstance<C>, Witness<C>), Error>
    where
        C: CurveGroup,
        C: CurveGroup<ScalarField = F>,
        <C as Group>::ScalarField: Absorb,
        CS: CommitmentScheme<C, true>,
    {
        let u = C::ScalarField::rand(&mut rng);
        let rE = C::ScalarField::rand(&mut rng);
        let rW = C::ScalarField::rand(&mut rng);

        let W = (0..self.A.n_cols - self.l - 1)
            .map(|_| F::rand(&mut rng))
            .collect();
        let x = (0..self.l).map(|_| F::rand(&mut rng)).collect::<Vec<F>>();
        let mut z = vec![u];
        z.extend(&x);
        z.extend(&W);

        let E = RelaxedR1CS::compute_E(&self.A, &self.B, &self.C, &z, &u)?;

        debug_assert!(
            z.len() == self.A.n_cols,
            "Length of z is {}, while A has {} columns.",
            z.len(),
            self.A.n_cols
        );

        debug_assert!(
            self.check_sampled_relaxed_r1cs(u, &E, &z),
            "Sampled a non satisfiable relaxed R1CS, sampled u: {}, computed E: {:?}",
            u,
            E
        );

        let witness = Witness { E, rE, W, rW };
        let mut cm_witness = witness.commit::<CS, true>(params, x)?;

        // witness.commit() sets u to 1, we set it to the sampled u value
        cm_witness.u = u;
        Ok((cm_witness, witness))
    }
}

/// extracts arkworks ConstraintSystem matrices into crate::utils::vec::SparseMatrix format as R1CS
/// struct.
pub fn extract_r1cs<F: PrimeField>(cs: &ConstraintSystem<F>) -> R1CS<F> {
    let m = cs.to_matrices().unwrap();

    let n_rows = cs.num_constraints;
    let n_cols = cs.num_instance_variables + cs.num_witness_variables; // cs.num_instance_variables already counts the 1

    let A = SparseMatrix::<F> {
        n_rows,
        n_cols,
        coeffs: m.a,
    };
    let B = SparseMatrix::<F> {
        n_rows,
        n_cols,
        coeffs: m.b,
    };
    let C = SparseMatrix::<F> {
        n_rows,
        n_cols,
        coeffs: m.c,
    };

    R1CS::<F> {
        l: cs.num_instance_variables - 1, // -1 to subtract the first '1'
        A,
        B,
        C,
    }
}

/// extracts the witness and the public inputs from arkworks ConstraintSystem.
pub fn extract_w_x<F: PrimeField>(cs: &ConstraintSystem<F>) -> (Vec<F>, Vec<F>) {
    (
        cs.witness_assignment.clone(),
        // skip the first element which is '1'
        cs.instance_assignment[1..].to_vec(),
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::{
        commitment::pedersen::Pedersen,
        utils::vec::{
            is_zero_vec,
            tests::{to_F_matrix, to_F_vec},
        },
    };

    use ark_pallas::{Fr, Projective};

    #[test]
    pub fn sample_relaxed_r1cs() {
        let rng = rand::rngs::OsRng;
        let r1cs = get_test_r1cs::<Fr>();
        let (prover_params, _) = Pedersen::<Projective>::setup(rng, r1cs.A.n_rows).unwrap();

        let relaxed_r1cs = r1cs.relax();
        let sampled =
            relaxed_r1cs.sample::<Projective, Pedersen<Projective, true>>(&prover_params, rng);
        assert!(sampled.is_ok());
    }

    pub fn get_test_r1cs<F: PrimeField>() -> R1CS<F> {
        // R1CS for: x^3 + x + 5 = y (example from article
        // https://www.vitalik.ca/general/2016/12/10/qap.html )
        let A = to_F_matrix::<F>(vec![
            vec![0, 1, 0, 0, 0, 0],
            vec![0, 0, 0, 1, 0, 0],
            vec![0, 1, 0, 0, 1, 0],
            vec![5, 0, 0, 0, 0, 1],
        ]);
        let B = to_F_matrix::<F>(vec![
            vec![0, 1, 0, 0, 0, 0],
            vec![0, 1, 0, 0, 0, 0],
            vec![1, 0, 0, 0, 0, 0],
            vec![1, 0, 0, 0, 0, 0],
        ]);
        let C = to_F_matrix::<F>(vec![
            vec![0, 0, 0, 1, 0, 0],
            vec![0, 0, 0, 0, 1, 0],
            vec![0, 0, 0, 0, 0, 1],
            vec![0, 0, 1, 0, 0, 0],
        ]);

        R1CS::<F> { l: 1, A, B, C }
    }

    pub fn get_test_z<F: PrimeField>(input: usize) -> Vec<F> {
        // z = (1, io, w)
        to_F_vec(vec![
            1,
            input,                             // io
            input * input * input + input + 5, // x^3 + x + 5
            input * input,                     // x^2
            input * input * input,             // x^2 * x
            input * input * input + input,     // x^3 + x
        ])
    }

    pub fn get_test_z_split<F: PrimeField>(input: usize) -> (F, Vec<F>, Vec<F>) {
        // z = (1, io, w)
        (
            F::one(),
            to_F_vec(vec![
                input, // io
            ]),
            to_F_vec(vec![
                input * input * input + input + 5, // x^3 + x + 5
                input * input,                     // x^2
                input * input * input,             // x^2 * x
                input * input * input + input,     // x^3 + x
            ]),
        )
    }

    #[test]
    fn test_eval_r1cs_relation() {
        let mut rng = ark_std::test_rng();
        let r1cs = get_test_r1cs::<Fr>();
        let mut z = get_test_z::<Fr>(rng.gen::<u16>() as usize);

        let f_w = r1cs.eval_relation(&z).unwrap();
        assert!(is_zero_vec(&f_w));

        z[1] = Fr::from(111);
        let f_w = r1cs.eval_relation(&z).unwrap();
        assert!(!is_zero_vec(&f_w));
    }

    #[test]
    fn test_check_r1cs_relation() {
        let r1cs = get_test_r1cs::<Fr>();
        let z = get_test_z(5);
        r1cs.check_relation(&z).unwrap();
    }
}
