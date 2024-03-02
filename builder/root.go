package builder

import (
	"math/big"

	"github.com/Zklib/gkr-compiler/expr"
	"github.com/consensys/gnark/constraint"
	bn254r1cs "github.com/consensys/gnark/constraint/bn254"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/schema"
)

type Root struct {
	*builder
	field  constraint.R1CS
	config frontend.CompileConfig

	registry *SubCircuitRegistry

	publicVariables []int
}

func NewRoot(field *big.Int, config frontend.CompileConfig) *Root {
	root := Root{
		config: config,
	}
	root.field = bn254r1cs.NewR1CS(config.Capacity)
	if field.Cmp(root.field.Field()) != 0 {
		panic("currently only BN254 is supported")
	}
	root.registry = newSubCircuitRegistry()

	root.builder = root.newBuilder(0)
	root.registry.m[0] = &SubCircuit{
		builder: root.builder,
	}

	return &root
}

// PublicVariable creates a new public Variable
func (r *Root) PublicVariable(f schema.LeafInfo) frontend.Variable {
	res := r.SecretVariable(f)
	r.publicVariables = append(r.publicVariables, res.(expr.Expression)[0].VID0)
	return res
}

// SecretVariable creates a new secret Variable
func (r *Root) SecretVariable(f schema.LeafInfo) frontend.Variable {
	r.builder.nbExternalInput++
	return expr.NewLinearExpression(r.newVariable(1), r.builder.tOne)
}
