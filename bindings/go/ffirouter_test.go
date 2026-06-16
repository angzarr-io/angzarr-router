//go:build ffirouter

package ffirouter

import "testing"

// The linked core must speak ABI version 1; a mismatch means the binding
// and the router-ffi artifact are out of step and nothing below can be
// trusted to marshal correctly.
func TestAbiVersion(t *testing.T) {
	if got := AbiVersion(); got != 1 {
		t.Fatalf("AbiVersion() = %d, want 1", got)
	}
}
