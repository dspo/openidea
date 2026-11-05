#!/bin/bash
# M8: generates the synthetic multi-package Go module used by the perf
# budget scenarios and the soak (>= 100 packages, real cross-package
# references so gopls does real work). Usage: gen-go-module.sh <dir> [packages]
set -euo pipefail
DIR="${1:?usage: gen-go-module.sh <dir> [packages]}"
N="${2:-100}"
mkdir -p "$DIR"
test -f "$DIR/go.mod" || (cd "$DIR" && go mod init m8gomodule)
for i in $(seq -w 1 "$N"); do
  P="$DIR/pkg$i"
  mkdir -p "$P"
  # Every package exposes Sym$i used by the next package (a real reference
  # chain), with a struct, an interface, and enough body for hints.
  cat > "$P/api.go" <<GO
package pkg$i

import "fmt"

// Sym$i is the package's exported handle (pkg$((10#$i + 1 > $N ? $N : 10#$i + 1)) consumes it).
type Sym$i struct {
	Name   string
	Values []int
}

// Provider feeds the next package in the chain.
type Provider interface {
	Provide(name string) *Sym$i
}

func New(name string) *Sym$i {
	s := &Sym$i{Name: name, Values: make([]int, 0, 8)}
	for k := range 8 {
		s.Values = append(s.Values, k*i)
	}
	return s
}

func (s *Sym$i) Render() string {
	total := 0
	for _, v := range s.Values {
		total += v
	}
	return fmt.Sprintf("%s: %d", s.Name, total)
}
GO
  NEXT=$((10#$i + 1))
  if [ "$NEXT" -le "$N" ]; then
    cat > "$P/use.go" <<GO
package pkg$i

import "m8gomodule/pkg$(printf '%03d' "$NEXT")"

// Use chains into the next package so the module has real cross-package edges.
func Use() string {
	helper := pkg$(printf '%03d' "$NEXT").New("from-$i")
	return helper.Render()
}
GO
  fi
done
echo "generated $N packages in $DIR"
