//go:build ffirouter

package ffirouter

import (
	"errors"
	"testing"
)

// newCounterRouter registers the CounterAggregate fixture and returns the
// router plus the handler's observation log.
func newCounterRouter(t *testing.T) (*Router, *[]observation) {
	t.Helper()
	r := NewRouter()
	t.Cleanup(r.Close)
	observed := &[]observation{}
	if err := RegisterAggregate(r, counterAggregate(observed)); err != nil {
		t.Fatalf("register: %v", err)
	}
	return r, observed
}

// A command emits one event per unit and stamps consecutive sequences —
// the full round trip across the seam: descriptor, dispatch, command
// callback, EventBook marshaling, and the core's sequence stamping.
func TestDispatch_IncreaseByEmitsSequencedEvents(t *testing.T) {
	r, _ := newCounterRouter(t)
	resp, err := r.Dispatch(increaseCommand(3))
	if err != nil {
		t.Fatalf("dispatch: %v", err)
	}
	book := resp.GetEvents()
	if book == nil {
		t.Fatal("expected an events result")
	}
	if len(book.Pages) != 3 {
		t.Fatalf("pages = %d, want 3", len(book.Pages))
	}
	for i, p := range book.Pages {
		if got := p.GetHeader().GetSequence(); got != uint32(i) {
			t.Errorf("page %d sequence = %d, want %d", i, got, i)
		}
	}
}

// A coded business rejection crosses back as a *CodedError carrying the
// stable reason — the google.rpc.Status/ErrorInfo round trip.
func TestDispatch_IncreaseByZeroIsCoded(t *testing.T) {
	r, _ := newCounterRouter(t)
	_, err := r.Dispatch(increaseCommand(0))
	var ce *CodedError
	if !errors.As(err, &ce) {
		t.Fatalf("err = %v (%T), want *CodedError", err, err)
	}
	if ce.Code != "VALUE_NOT_POSITIVE" {
		t.Errorf("code = %q, want VALUE_NOT_POSITIVE", ce.Code)
	}
}

// An unclassified handler error is classified by the binding as
// UNHANDLED_HANDLER_ERROR before it crosses the seam.
func TestDispatch_FailHardIsUnhandled(t *testing.T) {
	r, _ := newCounterRouter(t)
	_, derr := r.Dispatch(failHardCommand())
	var ce *CodedError
	if !errors.As(derr, &ce) || ce.Code != codeUnhandledHandlerError {
		t.Fatalf("err = %v, want UNHANDLED_HANDLER_ERROR", derr)
	}
}

// Prior events fold into state and reach the handler as historical
// evidence — the CommandContextAux decode plus applier execution across
// the seam. A fresh aggregate reports no prior history.
func TestDispatch_PriorEventsReachHandlerContext(t *testing.T) {
	r, observed := newCounterRouter(t)

	if _, err := r.Dispatch(increaseCommand(1)); err != nil {
		t.Fatalf("fresh dispatch: %v", err)
	}
	fresh := (*observed)[0]
	if fresh.cctx.HadPriorEvents {
		t.Error("fresh: HadPriorEvents = true, want false")
	}
	if fresh.cctx.NextSequence != 0 || fresh.count != 0 {
		t.Errorf("fresh: ctx=%+v count=%d, want next 0 / count 0", fresh.cctx, fresh.count)
	}

	cmd := increaseCommand(1)
	cmd.Events = priorIncreases(2)
	if _, err := r.Dispatch(cmd); err != nil {
		t.Fatalf("prior dispatch: %v", err)
	}
	prior := (*observed)[1]
	if !prior.cctx.HadPriorEvents {
		t.Error("prior: HadPriorEvents = false, want true")
	}
	if prior.cctx.NextSequence != 2 || prior.count != 2 {
		t.Errorf("prior: ctx=%+v count=%d, want next 2 / count 2", prior.cctx, prior.count)
	}
}
