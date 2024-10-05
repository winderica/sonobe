/// This file implements the onchain (Ethereum's EVM) decider circuit. For non-ethereum use cases,
/// other more efficient approaches can be used.
use ark_crypto_primitives::sponge::{
    constraints::CryptographicSpongeVar,
    poseidon::{constraints::PoseidonSpongeVar, PoseidonSponge},
    Absorb, CryptographicSponge,
};
use ark_ec::CurveGroup;
use ark_ff::PrimeField;
use ark_r1cs_std::{
    alloc::{AllocVar, AllocationMode},
    fields::fp::FpVar,
    groups::CurveVar,
    ToConstraintFieldGadget,
};
use ark_relations::r1cs::{Namespace, SynthesisError};
use ark_std::{borrow::Borrow, marker::PhantomData};

use crate::{
    arith::r1cs::{circuits::R1CSMatricesVar, R1CS},
    commitment::{pedersen::Params as PedersenParams, CommitmentScheme},
    folding::{
        circuits::{
            decider::{
                on_chain::GenericOnchainDeciderCircuit, DeciderEnabledNIFS, EvalGadget,
                KZGChallengesGadget,
            },
            CF1, CF2,
        },
        traits::{WitnessOps, WitnessVarOps},
    },
    frontend::FCircuit,
    Error,
};

use super::{
    circuits::FoldingGadget,
    constants::{INCOMING, RUNNING},
    folding::{Folding, ProtoGalaxyProof},
    CommittedInstance, CommittedInstanceVar, ProtoGalaxy, Witness,
};

/// In-circuit representation of the Witness associated to the CommittedInstance.
#[derive(Debug, Clone)]
pub struct WitnessVar<F: PrimeField> {
    pub W: Vec<FpVar<F>>,
    pub rW: FpVar<F>,
}

impl<F: PrimeField> AllocVar<Witness<F>, F> for WitnessVar<F> {
    fn new_variable<T: Borrow<Witness<F>>>(
        cs: impl Into<Namespace<F>>,
        f: impl FnOnce() -> Result<T, SynthesisError>,
        mode: AllocationMode,
    ) -> Result<Self, SynthesisError> {
        f().and_then(|val| {
            let cs = cs.into();

            let W = Vec::new_variable(cs.clone(), || Ok(val.borrow().w.to_vec()), mode)?;
            let rW = FpVar::new_variable(cs.clone(), || Ok(val.borrow().r_w), mode)?;

            Ok(Self { W, rW })
        })
    }
}

impl<F: PrimeField> WitnessVarOps<F> for WitnessVar<F> {
    fn get_openings(&self) -> Vec<(&[FpVar<F>], FpVar<F>)> {
        vec![(&self.W, self.rW.clone())]
    }
}

pub type DeciderEthCircuit<C1, C2, GC2> = GenericOnchainDeciderCircuit<
    C1,
    C2,
    GC2,
    CommittedInstance<C1, RUNNING>,
    CommittedInstance<C1, INCOMING>,
    Witness<CF1<C1>>,
    R1CS<CF1<C1>>,
    R1CSMatricesVar<CF1<C1>, FpVar<CF1<C1>>>,
    DeciderProtoGalaxyGadget,
>;

/// returns an instance of the DeciderEthCircuit from the given ProtoGalaxy struct
impl<
        C1: CurveGroup,
        GC1: CurveVar<C1, CF2<C1>> + ToConstraintFieldGadget<CF2<C1>>,
        C2: CurveGroup,
        GC2: CurveVar<C2, CF2<C2>> + ToConstraintFieldGadget<CF2<C2>>,
        FC: FCircuit<C1::ScalarField>,
        CS1: CommitmentScheme<C1, false>,
        // enforce that the CS2 is Pedersen commitment scheme, since we're at Ethereum's EVM decider
        CS2: CommitmentScheme<C2, false, ProverParams = PedersenParams<C2>>,
    > TryFrom<ProtoGalaxy<C1, GC1, C2, GC2, FC, CS1, CS2>> for DeciderEthCircuit<C1, C2, GC2>
where
    CF1<C1>: Absorb,
{
    type Error = Error;

    fn try_from(protogalaxy: ProtoGalaxy<C1, GC1, C2, GC2, FC, CS1, CS2>) -> Result<Self, Error> {
        let mut transcript = PoseidonSponge::<C1::ScalarField>::new(&protogalaxy.poseidon_config);

        let (U_i1, W_i1, proof, _) = Folding::prove(
            &mut transcript,
            &protogalaxy.r1cs,
            &protogalaxy.U_i,
            &protogalaxy.W_i,
            &[protogalaxy.u_i.clone()],
            &[protogalaxy.w_i.clone()],
        )?;

        // compute the KZG challenges used as inputs in the circuit
        let kzg_challenges = KZGChallengesGadget::get_challenges_native(&mut transcript, &U_i1);

        // get KZG evals
        let kzg_evaluations = W_i1
            .get_openings()
            .iter()
            .zip(&kzg_challenges)
            .map(|((v, _), &c)| EvalGadget::evaluate_native(v, c))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            _gc2: PhantomData,
            _avar: PhantomData,
            arith: protogalaxy.r1cs,
            cf_arith: protogalaxy.cf_r1cs,
            cf_pedersen_params: protogalaxy.cf_cs_params,
            poseidon_config: protogalaxy.poseidon_config,
            pp_hash: protogalaxy.pp_hash,
            i: protogalaxy.i,
            z_0: protogalaxy.z_0,
            z_i: protogalaxy.z_i,
            U_i: protogalaxy.U_i,
            W_i: protogalaxy.W_i,
            u_i: protogalaxy.u_i,
            w_i: protogalaxy.w_i,
            U_i1,
            W_i1,
            proof,
            cf_U_i: protogalaxy.cf_U_i,
            cf_W_i: protogalaxy.cf_W_i,
            kzg_challenges,
            kzg_evaluations,
        })
    }
}

pub struct DeciderProtoGalaxyGadget;

impl<C: CurveGroup>
    DeciderEnabledNIFS<
        C,
        CommittedInstance<C, RUNNING>,
        CommittedInstance<C, INCOMING>,
        Witness<CF1<C>>,
        R1CS<CF1<C>>,
    > for DeciderProtoGalaxyGadget
{
    type Proof = ProtoGalaxyProof<CF1<C>>;
    type ProofDummyCfg = (usize, usize, usize);

    fn fold_gadget(
        _arith: &R1CS<CF1<C>>,
        transcript: &mut PoseidonSpongeVar<CF1<C>>,
        _pp_hash: FpVar<CF1<C>>,
        U: CommittedInstanceVar<C, RUNNING>,
        _U_vec: Vec<FpVar<CF1<C>>>,
        u: CommittedInstanceVar<C, INCOMING>,
        proof: Self::Proof,
    ) -> Result<CommittedInstanceVar<C, RUNNING>, SynthesisError> {
        let cs = transcript.cs();
        let F_coeffs = Vec::new_witness(cs.clone(), || Ok(&proof.F_coeffs[..]))?;
        let K_coeffs = Vec::new_witness(cs.clone(), || Ok(&proof.K_coeffs[..]))?;

        Ok(FoldingGadget::fold_committed_instance(transcript, &U, &[u], F_coeffs, K_coeffs)?.0)
    }
}

#[cfg(test)]
pub mod tests {
    use ark_bn254::{constraints::GVar, Fr, G1Projective as Projective};
    use ark_grumpkin::{constraints::GVar as GVar2, Projective as Projective2};
    use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystem};

    use super::*;
    use crate::commitment::pedersen::Pedersen;
    use crate::folding::protogalaxy::ProtoGalaxy;
    use crate::frontend::{utils::CubicFCircuit, FCircuit};
    use crate::transcript::poseidon::poseidon_canonical_config;
    use crate::FoldingScheme;

    #[test]
    fn test_decider_circuit() {
        let mut rng = ark_std::test_rng();
        let poseidon_config = poseidon_canonical_config::<Fr>();

        let F_circuit = CubicFCircuit::<Fr>::new(()).unwrap();
        let z_0 = vec![Fr::from(3_u32)];

        type PG = ProtoGalaxy<
            Projective,
            GVar,
            Projective2,
            GVar2,
            CubicFCircuit<Fr>,
            Pedersen<Projective>,
            Pedersen<Projective2>,
        >;
        let pg_params = PG::preprocess(&mut rng, &(poseidon_config, F_circuit)).unwrap();

        // generate a Nova instance and do a step of it
        let mut protogalaxy = PG::init(&pg_params, F_circuit, z_0.clone()).unwrap();
        protogalaxy.prove_step(&mut rng, vec![], None).unwrap();

        let ivc_proof = protogalaxy.ivc_proof();
        PG::verify(pg_params.1, ivc_proof).unwrap();

        // load the DeciderEthCircuit from the generated Nova instance
        let decider_circuit =
            DeciderEthCircuit::<Projective, Projective2, GVar2>::try_from(protogalaxy).unwrap();

        let cs = ConstraintSystem::<Fr>::new_ref();

        // generate the constraints and check that are satisfied by the inputs
        decider_circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
        dbg!(cs.num_constraints());
    }
}