use core::panic;

use crate::circuit::ir::common::RawConstraint;
use crate::circuit::ir::expr;
use crate::field::Field;
use crate::{
    circuit::{
        config::Config,
        ir::{
            self,
            expr::{LinComb, LinCombTerm},
        },
        layered::Coef,
    },
    hints::BuiltinHintIds,
};

use super::basic::{process_root_circuit, InsnTransformAndExecute, RootBuilder};

type IrcIn<C> = ir::source::Irc<C>;
type IrcOut<C> = ir::hint_normalized::Irc<C>;
type InsnIn<C> = ir::source::Instruction<C>;
type InsnOut<C> = ir::hint_normalized::Instruction<C>;
type Builder<'a, C> = super::basic::Builder<'a, C, IrcIn<C>, IrcOut<C>>;

impl<'a, C: Config> Builder<'a, C> {
    fn push_const(&mut self, c: C::CircuitField) -> usize {
        self.push_insn(InsnOut::ConstantOrRandom(Coef::Constant(c)))
            .unwrap()
    }
    fn push_add(&mut self, a: usize, b: usize) -> usize {
        self.push_insn(InsnOut::LinComb(LinComb {
            terms: vec![
                LinCombTerm {
                    coef: C::CircuitField::one(),
                    var: a,
                },
                LinCombTerm {
                    coef: C::CircuitField::one(),
                    var: b,
                },
            ],
            constant: C::CircuitField::zero(),
        }))
        .unwrap()
    }
    fn push_sub(&mut self, a: usize, b: usize) -> usize {
        self.push_insn(InsnOut::LinComb(LinComb {
            terms: vec![
                LinCombTerm {
                    coef: C::CircuitField::one(),
                    var: a,
                },
                LinCombTerm {
                    coef: -C::CircuitField::one(),
                    var: b,
                },
            ],
            constant: C::CircuitField::zero(),
        }))
        .unwrap()
    }
    fn push_mul(&mut self, a: usize, b: usize) -> usize {
        self.push_insn(InsnOut::Mul(vec![a, b])).unwrap()
    }
    fn copy(&mut self, a: usize) -> InsnOut<C> {
        InsnOut::LinComb(LinComb {
            terms: vec![LinCombTerm {
                coef: C::CircuitField::one(),
                var: a,
            }],
            constant: C::CircuitField::zero(),
        })
    }
    fn bool_cond(&mut self, a: usize) -> usize {
        let one = self.push_const(C::CircuitField::one());
        let a_minus_one = self.push_sub(a, one);
        self.push_mul(a, a_minus_one)
    }
    fn assert_bool(&mut self, a: usize) {
        let t = self.bool_cond(a);
        self.assert((), t);
    }
    fn mark_bool(&mut self, a: usize) {
        let t = self.bool_cond(a);
        self.mark((), t);
    }
}

impl<'a, C: Config> InsnTransformAndExecute<'a, C, IrcIn<C>, IrcOut<C>> for Builder<'a, C> {
    fn transform_in_to_out(&mut self, in_insn: &InsnIn<C>) -> Result<InsnOut<C>, String> {
        use ir::source::Instruction::*;
        Ok(match in_insn {
            LinComb(lcs) => InsnOut::LinComb(lcs.clone()),
            Mul(vars) => InsnOut::Mul(vars.clone()),
            Div { x, y, checked } => match self.constant_value(*y) {
                Some(yv) => {
                    if yv.is_zero() {
                        return Err("division by zero constant".to_string());
                    }
                    let y = self.push_const(yv.inv().unwrap());
                    InsnOut::Mul(vec![*x, y])
                }
                None => {
                    if *checked {
                        let one = self.push_const(C::CircuitField::one());
                        let inv = self
                            .push_insn(InsnOut::Hint {
                                hint_id: BuiltinHintIds::Div as usize,
                                inputs: vec![one, *y],
                                num_outputs: 1,
                            })
                            .unwrap();
                        let multy = self.push_mul(*y, inv);
                        let sub1 = self.push_sub(multy, one);
                        self.assert((), sub1);
                        InsnOut::Mul(vec![*x, inv])
                    } else {
                        let div_res = self
                            .push_insn(InsnOut::Hint {
                                hint_id: BuiltinHintIds::Div as usize,
                                inputs: vec![*x, *y],
                                num_outputs: 1,
                            })
                            .unwrap();
                        let multy = self.push_mul(*y, div_res);
                        let subx = self.push_sub(multy, *x);
                        self.assert((), subx);
                        self.copy(div_res)
                    }
                }
            },
            BoolBinOp { x, y, op } => {
                self.assert_bool(*x);
                self.assert_bool(*y);
                let x_plus_y = self.push_add(*x, *y);
                let x_times_y = self.push_mul(*x, *y);
                let res = match op {
                    ir::source::BoolBinOpType::And => x_times_y,
                    ir::source::BoolBinOpType::Or => self.push_sub(x_plus_y, x_times_y),
                    ir::source::BoolBinOpType::Xor => self
                        .push_insn(InsnOut::LinComb(expr::LinComb {
                            terms: vec![
                                LinCombTerm {
                                    coef: C::CircuitField::one(),
                                    var: x_plus_y,
                                },
                                LinCombTerm {
                                    coef: C::CircuitField::one() + C::CircuitField::one(),
                                    var: x_times_y,
                                },
                            ],
                            constant: C::CircuitField::zero(),
                        }))
                        .unwrap(),
                };
                self.mark_bool(res);
                self.copy(res)
            }
            IsZero(x) => {
                if let Some(xv) = self.constant_value(*x) {
                    InsnOut::ConstantOrRandom(Coef::Constant(if xv.is_zero() {
                        C::CircuitField::one()
                    } else {
                        C::CircuitField::zero()
                    }))
                } else {
                    let one = self.push_const(C::CircuitField::one());
                    let inv = self
                        .push_insn(InsnOut::Hint {
                            hint_id: BuiltinHintIds::Div as usize,
                            inputs: vec![one, *x],
                            num_outputs: 1,
                        })
                        .unwrap();
                    let prod = self.push_mul(*x, inv);
                    let m = self.push_sub(one, prod);
                    let xm = self.push_mul(*x, m);
                    self.assert((), xm);
                    self.mark_bool(m);
                    self.copy(m)
                }
            }
            Commit(_) => {
                panic!("commit is unimplemented");
            }
            Hint {
                hint_id,
                inputs,
                num_outputs,
            } => ir::hint_normalized::Instruction::Hint {
                hint_id: *hint_id,
                inputs: inputs.clone(),
                num_outputs: *num_outputs,
            },
            ConstantOrRandom(coef) => {
                ir::hint_normalized::Instruction::ConstantOrRandom(coef.clone())
            }
            SubCircuitCall {
                sub_circuit_id,
                inputs,
                num_outputs,
            } => ir::hint_normalized::Instruction::SubCircuitCall {
                sub_circuit_id: *sub_circuit_id,
                inputs: inputs.clone(),
                num_outputs: *num_outputs,
            },
            UnconstrainedBinOp { x, y, op } => {
                let xc = self.constant_value(*x);
                let yc = self.constant_value(*y);
                if let (Some(xv), Some(yv)) = (&xc, &yc) {
                    InsnOut::ConstantOrRandom(Coef::Constant(op.eval(xv, yv)?))
                } else {
                    use ir::source::UnconstrainedBinOpType::*;
                    let op = match op {
                        Div => BuiltinHintIds::Div,
                        Pow => BuiltinHintIds::Pow,
                        IntDiv => BuiltinHintIds::IntDiv,
                        Mod => BuiltinHintIds::Mod,
                        ShiftL => BuiltinHintIds::ShiftL,
                        ShiftR => BuiltinHintIds::ShiftR,
                        LesserEq => BuiltinHintIds::LesserEq,
                        GreaterEq => BuiltinHintIds::GreaterEq,
                        Lesser => BuiltinHintIds::Lesser,
                        Greater => BuiltinHintIds::Greater,
                        Eq => BuiltinHintIds::Eq,
                        NotEq => BuiltinHintIds::NotEq,
                        BoolOr => BuiltinHintIds::BoolOr,
                        BoolAnd => BuiltinHintIds::BoolAnd,
                        BitOr => BuiltinHintIds::BitOr,
                        BitAnd => BuiltinHintIds::BitAnd,
                        BitXor => BuiltinHintIds::BitXor,
                    };
                    InsnOut::Hint {
                        hint_id: op as usize,
                        inputs: vec![*x, *y],
                        num_outputs: 1,
                    }
                }
            }
            UnconstrainedSelect {
                cond,
                if_true,
                if_false,
            } => {
                if let Some(cond) = self.constant_value(*cond) {
                    if cond.is_zero() {
                        self.copy(*if_false)
                    } else {
                        self.copy(*if_true)
                    }
                } else {
                    InsnOut::Hint {
                        hint_id: BuiltinHintIds::Select as usize,
                        inputs: vec![*cond, *if_true, *if_false],
                        num_outputs: 1,
                    }
                }
            }
        })
    }

    fn transform_in_con_to_out(
        &mut self,
        in_con: &ir::source::Constraint,
    ) -> Result<RawConstraint, String> {
        match in_con.typ {
            ir::source::ConstraintType::Zero => Ok(in_con.var),
            ir::source::ConstraintType::Bool => Ok(self.bool_cond(in_con.var)),
            ir::source::ConstraintType::NonZero => {
                let one = self.push_const(C::CircuitField::one());
                let inv = self
                    .push_insn(InsnOut::Hint {
                        hint_id: BuiltinHintIds::Div as usize,
                        inputs: vec![one, in_con.var],
                        num_outputs: 1,
                    })
                    .unwrap();
                let multy = self.push_mul(in_con.var, inv);
                let sub1 = self.push_sub(multy, one);
                Ok(sub1)
            }
        }
    }

    fn execute_out<'b>(
        &mut self,
        out_insn: &InsnOut<C>,
        root: Option<&'b RootBuilder<'a, C, IrcIn<C>, IrcOut<C>>>,
    ) where
        'a: 'b,
    {
        match out_insn {
            InsnOut::LinComb(lc) => {
                self.add_lin_comb(lc);
            }
            InsnOut::Mul(inputs) => {
                self.add_mul_vec(inputs.clone());
            }
            InsnOut::Hint { num_outputs, .. } => {
                self.add_out_vars(*num_outputs);
            }
            InsnOut::ConstantOrRandom(coef) => match coef {
                Coef::Constant(c) => {
                    self.add_const(*c);
                }
                Coef::Random => {
                    self.add_out_vars(1);
                }
            },
            InsnOut::SubCircuitCall {
                sub_circuit_id,
                inputs,
                num_outputs,
            } => {
                self.sub_circuit_call(*sub_circuit_id, inputs, *num_outputs, root);
            }
        }
    }
}

pub fn process<C: Config>(
    rc: &ir::common::RootCircuit<IrcIn<C>>,
) -> Result<ir::common::RootCircuit<IrcOut<C>>, String> {
    process_root_circuit(rc)
}

#[cfg(test)]
mod tests {
    use crate::circuit::{
        config::{Config, M31Config as C},
        ir::{self, common::rand_gen::*},
    };
    use crate::field::Field;

    type CField = <C as Config>::CircuitField;

    #[test]
    fn simple_invariant() {
        let mut root = ir::common::RootCircuit::<super::IrcIn<C>>::default();
        let lc = ir::expr::LinComb {
            terms: vec![
                ir::expr::LinCombTerm {
                    coef: CField::one(),
                    var: 1,
                },
                ir::expr::LinCombTerm {
                    coef: CField::one(),
                    var: 2,
                },
            ],
            constant: CField::one(),
        };
        root.circuits.insert(
            0,
            ir::common::Circuit::<super::IrcIn<C>> {
                instructions: vec![ir::source::Instruction::LinComb(lc.clone())],
                constraints: vec![ir::source::Constraint {
                    typ: ir::source::ConstraintType::Zero,
                    var: 3,
                }],
                outputs: vec![],
                num_inputs: 2,
                num_hint_inputs: 0,
            },
        );
        assert_eq!(root.validate(), Ok(()));
        let root_processed = super::process(&root).unwrap();
        assert_eq!(root_processed.validate(), Ok(()));
        match &root_processed.circuits[&0].instructions[0] {
            ir::hint_normalized::Instruction::LinComb(lc2) => {
                assert_eq!(lc, *lc2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn random_circuits_1() {
        let mut config = RandomCircuitConfig {
            seed: 0,
            num_circuits: RandomRange { min: 1, max: 10 },
            num_inputs: RandomRange { min: 1, max: 10 },
            num_hint_inputs: RandomRange { min: 0, max: 10 },
            num_instructions: RandomRange { min: 1, max: 10 },
            num_constraints: RandomRange { min: 0, max: 10 },
            num_outputs: RandomRange { min: 1, max: 10 },
            num_terms: RandomRange { min: 1, max: 5 },
            sub_circuit_prob: 0.5,
        };
        for i in 0..3000 {
            config.seed = i + 200000;
            let root = ir::common::RootCircuit::<super::IrcIn<C>>::random(&config);
            assert_eq!(root.validate(), Ok(()));
            if let Ok(root_processed) = super::process(&root) {
                assert_eq!(root_processed.validate(), Ok(()));
                assert_eq!(root.input_size(), root_processed.input_size());
                for _ in 0..5 {
                    let inputs: Vec<CField> = (0..root.input_size())
                        .map(|_| CField::random_unsafe())
                        .collect();
                    let e1 = root.eval_unsafe_with_errors(inputs.clone());
                    let e2 = root_processed.eval_unsafe_with_errors(inputs);
                    if e1.is_ok() {
                        assert_eq!(e2, e1);
                    }
                }
            }
        }
    }

    #[test]
    fn random_circuits_2() {
        let mut config = RandomCircuitConfig {
            seed: 0,
            num_circuits: RandomRange { min: 1, max: 20 },
            num_inputs: RandomRange { min: 1, max: 3 },
            num_hint_inputs: RandomRange { min: 0, max: 2 },
            num_instructions: RandomRange { min: 30, max: 50 },
            num_constraints: RandomRange { min: 0, max: 5 },
            num_outputs: RandomRange { min: 1, max: 3 },
            num_terms: RandomRange { min: 1, max: 5 },
            sub_circuit_prob: 0.05,
        };
        for i in 0..1000 {
            config.seed = i + 300000;
            let root = ir::common::RootCircuit::<super::IrcIn<C>>::random(&config);
            assert_eq!(root.validate(), Ok(()));
            if let Ok(root_processed) = super::process(&root) {
                assert_eq!(root_processed.validate(), Ok(()));
                assert_eq!(root.input_size(), root_processed.input_size());
                for _ in 0..5 {
                    let inputs: Vec<CField> = (0..root.input_size())
                        .map(|_| CField::random_unsafe())
                        .collect();
                    let e1 = root.eval_unsafe_with_errors(inputs.clone());
                    let e2 = root_processed.eval_unsafe_with_errors(inputs);
                    if e1.is_ok() {
                        assert_eq!(e2, e1);
                    }
                }
            }
        }
    }
}
