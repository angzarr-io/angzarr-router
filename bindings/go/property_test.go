//go:build ffirouter

package ffirouter

import (
	"errors"
	"testing"
)

// TestPropertySweep exercises the core's sequence stamping and rejection
// threshold across a broad grid of (prior history, increase amount) the
// named scenarios don't enumerate. The expected outcome is the obvious
// reference model — amount events stamped at consecutive sequences
// continuing from the prior count, or a VALUE_NOT_POSITIVE rejection at
// amount 0 — so a divergence flags an off-by-one in continuation or a wrong
// threshold. No old client is linked; the shared conformance features
// (steps_test.go) remain the cross-language behavior contract.
func TestPropertySweep(t *testing.T) {
	r := NewRouter()
	defer r.Close()
	if err := RegisterAggregate(r, counterAggregate(nil)); err != nil {
		t.Fatalf("register: %v", err)
	}

	const bound = 13 // 13 x 13 = 169 (prior, amount) combinations
	for priorN := uint32(0); priorN < bound; priorN++ {
		for amount := uint32(0); amount < bound; amount++ {
			cmd := increaseCommand(amount)
			cmd.Events = priorIncreases(priorN)
			resp, err := r.Dispatch(cmd)

			if amount == 0 {
				var ce *CodedError
				if !errors.As(err, &ce) || ce.Code != "VALUE_NOT_POSITIVE" {
					t.Errorf("prior=%d amount=0: err=%v, want VALUE_NOT_POSITIVE", priorN, err)
				}
				continue
			}
			if err != nil {
				t.Errorf("prior=%d amount=%d: unexpected error %v", priorN, amount, err)
				continue
			}
			pages := resp.GetEvents().GetPages()
			if uint32(len(pages)) != amount {
				t.Errorf("prior=%d amount=%d: %d events, want %d", priorN, amount, len(pages), amount)
				continue
			}
			for i, p := range pages {
				if want, got := priorN+uint32(i), p.GetHeader().GetSequence(); got != want {
					t.Errorf("prior=%d amount=%d: event %d at seq %d, want %d", priorN, amount, i, got, want)
				}
			}
		}
	}
}
