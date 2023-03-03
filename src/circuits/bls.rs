use std::{marker::PhantomData, ops::Add};
use std::sync::Arc;
use halo2ecc_s::circuit::base_chip::BaseChipOps;
use halo2_proofs::{
    arithmetic::{FieldExt, BaseExt},
    circuit::{AssignedCell, Chip, Layouter, Region},
    plonk::{Advice, Fixed, Column, ConstraintSystem, Error},
    poly::Rotation,
    pairing::bls12_381::{G1Affine, G2Affine, G1, G2 }
};
use ark_std::{end_timer, start_timer};
use halo2_proofs::pairing::bn256::Fr;
use std::rc::Rc;
use std::cell::RefCell;

use halo2_proofs::pairing::bls12_381::pairing;
use halo2_proofs::pairing::bls12_381::Fq as Bls381Fq;
use halo2ecc_s::circuit::ecc_chip::EccBaseIntegerChipWrapper;
use halo2ecc_s::assign::{
    AssignedCondition,
    AssignedG1Affine,
    Cell as ContextCell, AssignedFq
};
/*
use halo2ecc_s::circuit::fq12::Fq12ChipOps;
use halo2ecc_s::circuit::fq12::Fq2ChipOps;
use halo2ecc_s::circuit::base_chip::BaseChipOps;
use halo2ecc_s::circuit::ecc_chip::EccChipBaseOps;
use halo2_proofs::pairing::group::prime::PrimeCurveAffine;
use halo2_proofs::pairing::group::Group;
*/
use halo2ecc_s::circuit::pairing_chip::PairingChipOps;
use halo2ecc_s::assign::{
    AssignedPoint,
    AssignedG2Affine,
    AssignedFq12,
};

use halo2ecc_s::{
    circuit::{
        base_chip::{BaseChip, BaseChipConfig},
        range_chip::{RangeChip, RangeChipConfig},
    },
    context::{Context, Records, GeneralScalarEccContext},
};

use crate::utils::{field_to_bn, bn_to_field};
use num_bigint::BigUint;
use std::ops::{Mul, AddAssign};


#[derive(Clone, Debug)]
pub struct Bls381ChipConfig {
    base_chip_config: BaseChipConfig,
    range_chip_config: RangeChipConfig,
}


#[derive(Clone, Debug)]
pub struct Bls381PairChip<N: FieldExt> {
    config: Bls381ChipConfig,
    _marker: PhantomData<N>,
}

impl<N: FieldExt> Chip<N> for Bls381PairChip<N> {
    type Config = Bls381ChipConfig;
    type Loaded = ();

    fn config(&self) -> &Self::Config {
        &self.config
    }

    fn loaded(&self) -> &Self::Loaded {
        &()
    }
}

pub fn fr_to_bn(f: &Fr) -> BigUint {
    let mut bytes: Vec<u8> = Vec::new();
    f.write(&mut bytes).unwrap();
    BigUint::from_bytes_le(&bytes[..])
}

pub fn fr_to_bool(f: &Fr) -> bool {
    let mut bytes: Vec<u8> = Vec::new();
    f.write(&mut bytes).unwrap();
    return bytes[0] == 1u8;
}

fn assigned_cells_to_bn381 (
    a:&Vec<AssignedCell<Fr, Fr>>, //G1 (4 * 2 + 1)
    start: usize
) -> BigUint {
    let mut bn = BigUint::from(0 as u64);
    for i in start..start + 4 {
        let shift = BigUint::from(2 as u32).pow(108 * (i - start) as u32);
        bn.add_assign(fr_to_bn(a[i].value().unwrap()).mul(shift.clone()));
    }
    bn
}

fn get_g1_from_cells(
    ctx: &mut GeneralScalarEccContext<G1Affine, Fr>,
    a:&Vec<AssignedCell<Fr, Fr>>, //G1 (4 * 2 + 1)
) -> AssignedPoint<G1Affine, Fr> {
    let x_bn = assigned_cells_to_bn381(a, 0);
    let y_bn = assigned_cells_to_bn381(a, 4);
    let is_identity = fr_to_bool(a[8].value().unwrap());
    let x = ctx.base_integer_chip().assign_w(&x_bn);
    let y = ctx.base_integer_chip().assign_w(&y_bn);
    AssignedPoint::new(
        x,
        y,
        AssignedCondition(ctx.native_ctx.borrow_mut().assign(
            if is_identity { Fr::one() } else { Fr::zero() }
        ))
    )
}

fn get_g2_from_cells(
    ctx: &mut GeneralScalarEccContext<G1Affine, Fr>,
    b:&Vec<AssignedCell<Fr, Fr>>, //G2 (4 * 4 + 1)
) -> AssignedG2Affine<G1Affine, Fr> {
    let x1_bn = assigned_cells_to_bn381(b, 0);
    let x2_bn = assigned_cells_to_bn381(b, 4);
    let y1_bn = assigned_cells_to_bn381(b, 8);
    let y2_bn = assigned_cells_to_bn381(b, 12);
    let x1 = ctx.base_integer_chip().assign_w(&x1_bn);
    let x2 = ctx.base_integer_chip().assign_w(&x2_bn);
    let y1 = ctx.base_integer_chip().assign_w(&y1_bn);
    let y2 = ctx.base_integer_chip().assign_w(&y2_bn);
    let is_identity = fr_to_bool(b[16].value().unwrap());
    AssignedG2Affine::new(
        (x1, x2),
        (y1, y2),
        AssignedCondition(ctx.native_ctx.borrow_mut().assign(
            if is_identity { Fr::one() } else { Fr::zero() }
        ))
    )
}

fn get_cell_of_ctx(
    cells: &Vec<Vec<Vec<Option<AssignedCell<Fr, Fr>>>>>,
    cell: &ContextCell,
) -> AssignedCell<Fr, Fr> {
    cells[cell.region as usize][cell.col][cell.row].clone().unwrap()
}

fn enable_fq_permute(
    region: &mut Region<'_, Fr>,
    cells: &Vec<Vec<Vec<Option<AssignedCell<Fr, Fr>>>>>,
    fq: &AssignedFq<Bls381Fq, Fr>,
    input: &Vec<AssignedCell<Fr, Fr>>
) -> Result<(), Error> {
    for i in 0..4 {
        let limb = fq.limbs_le[i].cell;
        let limb_assigned = get_cell_of_ctx(cells, &limb);
        region.constrain_equal(input[0].cell(), limb_assigned.cell())?;
    }
    Ok(())
}

fn enable_g1affine_permute(
    region: &mut Region<'_, Fr>,
    cells: &Vec<Vec<Vec<Option<AssignedCell<Fr, Fr>>>>>,
    point: &AssignedPoint<G1Affine, Fr>,
    input: &Vec<AssignedCell<Fr, Fr>>
) -> Result<(), Error> {
    for i in 0..4 {
        let x_limb0 = point.x.limbs_le[i].cell;
        let y_limb0 = point.y.limbs_le[i].cell;
        let x_limb0_assigned = get_cell_of_ctx(cells, &x_limb0);
        let y_limb0_assigned = get_cell_of_ctx(cells, &y_limb0);
        region.constrain_equal(input[i].cell(), x_limb0_assigned.cell())?;
        region.constrain_equal(input[i + 4].cell(), y_limb0_assigned.cell())?;
    }
    let z_limb0 = point.z.0.cell;
    let z_limb0_assigned = get_cell_of_ctx(cells, &z_limb0);
    region.constrain_equal(input[8].cell(), z_limb0_assigned.cell())?;
    Ok(())
}

fn enable_g2affine_permute(
    region: &mut Region<'_, Fr>,
    cells: &Vec<Vec<Vec<Option<AssignedCell<Fr, Fr>>>>>,
    point: &AssignedG2Affine<G1Affine, Fr>,
    input: &Vec<AssignedCell<Fr, Fr>>
) -> Result<(), Error> {
    let mut inputs = input.chunks(4);
    enable_fq_permute(region, cells, &point.x.0, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &point.x.1, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &point.y.0, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &point.y.1, &inputs.next().unwrap().to_vec())?;
    let z_limb0 = point.z.0.cell;
    let z_limb0_assigned = get_cell_of_ctx(cells, &z_limb0);
    region.constrain_equal(input[16].cell(), z_limb0_assigned.cell())?;
    Ok(())
}

fn enable_fq12_permute(
    region: &mut Region<'_, Fr>,
    cells: &Vec<Vec<Vec<Option<AssignedCell<Fr, Fr>>>>>,
    fq12: &AssignedFq12<Bls381Fq, Fr>,
    input: &Vec<AssignedCell<Fr, Fr>>
) -> Result<(), Error> {
    let mut inputs = input.chunks(4);
    enable_fq_permute(region, cells, &fq12.0.0.0, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.0.0.1, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.0.1.0, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.0.1.1, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.0.2.0, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.0.2.1, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.1.0.0, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.1.0.1, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.1.1.0, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.1.1.1, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.1.2.0, &inputs.next().unwrap().to_vec())?;
    enable_fq_permute(region, cells, &fq12.1.2.1, &inputs.next().unwrap().to_vec())?;
    Ok(())
}

impl Bls381PairChip<Fr> {
    pub fn construct(config: <Self as Chip<Fr>>::Config) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }

    pub fn configure(
        meta: &mut ConstraintSystem<Fr>,
        base_chip_config: BaseChipConfig,
        range_chip_config: RangeChipConfig,
    ) -> <Self as Chip<Fr>>::Config {
        Bls381ChipConfig {
            base_chip_config,
            range_chip_config,
        }
    }

    pub fn load_bls381_pair_circuit(
        &self,
        a: &Vec<AssignedCell<Fr, Fr>>, //G1 (4 * 2 + 1)
        b: &Vec<AssignedCell<Fr, Fr>>, //G2 (4 * 4 + 1)
        ab: &Vec<AssignedCell<Fr, Fr>>, // Fq_12 (4 * 12)
        base_chip: &BaseChip<Fr>,
        range_chip: &RangeChip<Fr>,
        mut layouter: impl Layouter<Fr>,
    ) -> Result<(), Error> {
        let contex = Rc::new(RefCell::new(Context::new()));
        let mut ctx = GeneralScalarEccContext::<G1Affine, Fr>::new(contex);

        let a_g1 = get_g1_from_cells(&mut ctx, a);
        let b_g2 = get_g2_from_cells(&mut ctx, b);

        //records.enable_permute(cell);

        let ab_fq12 = ctx.pairing(&[(&a_g1, &b_g2)]);

        //println!("x: {:?}, y: {:?}", a_g1.x.limbs_le, a_g1.y.limbs_le);
        /*
        ctx.fq12_assert_eq(&ab0, &ab);

        run_circuit_on_bn256(ctx.into(), 22);
        */
        let mut records = Arc::try_unwrap(Into::<Context<Fr>>::into(ctx).records).unwrap().into_inner().unwrap();

        //records.enable_permute(&ab.0.0.0.limbs_le[0].cell);

        layouter.assign_region(
            || "base",
            |mut region| {
                let timer = start_timer!(|| "assign");
                let cells = records
                    .assign_all(&mut region, &base_chip, &range_chip)?;
                enable_g1affine_permute(&mut region, &cells, &a_g1, a)?;
          //    enable_g2affine_permute(&mut region, &cells, &b_g2, b);
                enable_fq12_permute(&mut region, &cells, &ab_fq12, ab);
                end_timer!(timer);
                Ok(())
            },
        )?;
        Ok(())
    }
}

impl super::HostOpSelector for Bls381PairChip<Fr> {
    fn assign(
        layouter: &mut impl Layouter<Fr>,
        filtered_operands: Column<Advice>,
        filtered_opcodes: Column<Advice>,
        filtered_index: Column<Advice>,
        merged_operands: Column<Advice>,
        indicator: Column<Fixed>,
        offset: &mut usize,
        args: [Fr; 3],
    ) -> Result<Vec<AssignedCell<Fr, Fr>>, Error> {
        todo!()
    }
}