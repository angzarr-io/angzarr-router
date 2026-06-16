//go:build ffirouter

package ffirouter

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"

	"github.com/cucumber/godog"
	"google.golang.org/protobuf/proto"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// TestConformance runs the shared cross-language behavior suite — the same
// conformance/features + conformance/fixtures the Rust cucumber-rs harness
// runs — against the Go binding via godog. Only the step layer is new.
func TestConformance(t *testing.T) {
	suite := godog.TestSuite{
		ScenarioInitializer: initializeScenario,
		Options: &godog.Options{
			Format:   "pretty",
			Paths:    []string{"../../conformance/features"},
			TestingT: t,
			Strict:   true,
		},
	}
	if suite.Run() != 0 {
		t.Fatal("conformance scenarios failed")
	}
}

// counterWorld holds one scenario's state: a router with the fixture
// registered, the prior-events book to supply, and the dispatch outcome.
type counterWorld struct {
	router   *Router
	prior    *pb.EventBook
	resp     *pb.BusinessResponse
	err      error
	observed *[]observation
}

func (w *counterWorld) reset() {
	if w.router != nil {
		w.router.Close()
	}
	w.router = NewRouter()
	w.observed = &[]observation{}
	w.prior = nil
	w.resp = nil
	w.err = nil
	if err := RegisterAggregate(w.router, counterAggregate(w.observed)); err != nil {
		panic(fmt.Sprintf("register fixture: %v", err))
	}
}

func (w *counterWorld) dispatch(cc *pb.ContextualCommand) {
	cc.Events = w.prior
	w.resp, w.err = w.router.Dispatch(cc)
}

// --- Given: the prior-history the next dispatch rebuilds over ---

func (w *counterWorld) aNewCounter()            { w.prior = nil }
func (w *counterWorld) recordedIncreases(n int) { w.prior = priorIncreases(uint32(n)) }
func (w *counterWorld) historyHoldsCorrupt()    { w.prior = corruptHistory() }
func (w *counterWorld) restoredFromSnapshot()   { w.prior = snapshotHistory() }

// --- When: dispatch a command ---

func (w *counterWorld) increaseBy(n int)       { w.dispatch(increaseCommand(uint32(n))) }
func (w *counterWorld) increaseOnBehalf(n int) { w.dispatch(increaseCommandWithLinkage(uint32(n))) }
func (w *counterWorld) triggerHardFailure()    { w.dispatch(failHardCommand()) }
func (w *counterWorld) unhandledDispatched()   { w.dispatch(unhandledCommand()) }
func (w *counterWorld) reserveRejected()       { w.dispatch(rejectionCommand("test.counter.Reserve")) }
func (w *counterWorld) unregisteredRejected() {
	w.dispatch(rejectionCommand("test.counter.Undeclared"))
}
func (w *counterWorld) commandNoBook()    { w.dispatch(commandMissingBook()) }
func (w *counterWorld) commandEmptyBook() { w.dispatch(commandMissingPage()) }
func (w *counterWorld) commandNoPayload() { w.dispatch(commandMissingPayload()) }

// --- Then: assert the outcome ---

// recorded checks the emitted events: count pages at consecutive sequences
// starting from start. Serves both "starting at" and "continuing from".
func (w *counterWorld) recorded(count, start int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	book := w.resp.GetEvents()
	if book == nil {
		return errors.New("expected an events result, got none")
	}
	if len(book.Pages) != count {
		return fmt.Errorf("recorded %d events, want %d", len(book.Pages), count)
	}
	for i, p := range book.Pages {
		if got := int(p.GetHeader().GetSequence()); got != start+i {
			return fmt.Errorf("event %d at sequence %d, want %d", i, got, start+i)
		}
	}
	return nil
}

// failsWith asserts the dispatch failed with a specific coded reason.
func (w *counterWorld) failsWith(code string) error {
	var ce *CodedError
	if !errors.As(w.err, &ce) {
		return fmt.Errorf("expected coded error %s, got %v", code, w.err)
	}
	if ce.Code != code {
		return fmt.Errorf("code = %s, want %s", ce.Code, code)
	}
	return nil
}

func (w *counterWorld) noEventsRecorded() error {
	if n := len(w.resp.GetEvents().GetPages()); n != 0 {
		return fmt.Errorf("expected no events, got %d", n)
	}
	return nil
}

func (w *counterWorld) eventsCarryParentLinkage() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	ext := w.resp.GetEvents().GetCover().GetExt()
	if ext == nil {
		return errors.New("emitted events carry no cover ext")
	}
	if !proto.Equal(ext, parentLinkage()) {
		return fmt.Errorf("cover ext = %v, want parent linkage", ext)
	}
	return nil
}

func (w *counterWorld) compensationsFirstThenSecond() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	book := w.resp.GetEvents()
	want := []string{"test.counter.CompensatedFirst", "test.counter.CompensatedSecond"}
	if book == nil || len(book.Pages) != len(want) {
		return fmt.Errorf("expected %d compensation events, got %v", len(want), book)
	}
	for i, p := range book.Pages {
		if got := fqFromURL(p.GetEvent().GetTypeUrl()); got != want[i] {
			return fmt.Errorf("compensation %d = %s, want %s", i, got, want[i])
		}
	}
	return nil
}

func (w *counterWorld) noCompensation() error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if n := len(w.resp.GetEvents().GetPages()); n != 0 {
		return fmt.Errorf("expected no compensation events, got %d", n)
	}
	return nil
}

// handlerSawHistory asserts the historical-state evidence the handler
// observed; noPrior is "no " for a fresh aggregate, empty otherwise.
func (w *counterWorld) handlerSawHistory(noPrior string, nextSeq int) error {
	obs, err := w.lastObserved()
	if err != nil {
		return err
	}
	wantPrior := noPrior == ""
	if obs.cctx.HadPriorEvents != wantPrior {
		return fmt.Errorf("HadPriorEvents = %v, want %v", obs.cctx.HadPriorEvents, wantPrior)
	}
	if int(obs.cctx.NextSequence) != nextSeq {
		return fmt.Errorf("NextSequence = %d, want %d", obs.cctx.NextSequence, nextSeq)
	}
	return nil
}

func (w *counterWorld) handlerSawCounter(count, nextSeq int) error {
	obs, err := w.lastObserved()
	if err != nil {
		return err
	}
	if int(obs.count) != count {
		return fmt.Errorf("observed counter = %d, want %d", obs.count, count)
	}
	if int(obs.cctx.NextSequence) != nextSeq {
		return fmt.Errorf("NextSequence = %d, want %d", obs.cctx.NextSequence, nextSeq)
	}
	return nil
}

func (w *counterWorld) lastObserved() (observation, error) {
	if w.err != nil {
		return observation{}, fmt.Errorf("dispatch failed: %w", w.err)
	}
	if len(*w.observed) == 0 {
		return observation{}, errors.New("the handler recorded no observation")
	}
	return (*w.observed)[len(*w.observed)-1], nil
}

func fqFromURL(u string) string {
	if i := strings.LastIndex(u, "/"); i >= 0 {
		return u[i+1:]
	}
	return u
}

func initializeScenario(sc *godog.ScenarioContext) {
	w := &counterWorld{}
	sc.Before(func(ctx context.Context, _ *godog.Scenario) (context.Context, error) {
		w.reset()
		return ctx, nil
	})
	sc.After(func(ctx context.Context, _ *godog.Scenario, _ error) (context.Context, error) {
		w.router.Close()
		return ctx, nil
	})

	sc.Step(`^a new counter$`, w.aNewCounter)
	sc.Step(`^a counter that has already recorded (\d+) increases?$`, w.recordedIncreases)
	sc.Step(`^a counter whose history holds a corrupt event$`, w.historyHoldsCorrupt)
	sc.Step(`^a counter restored from a snapshot of 10 with one newer event$`, w.restoredFromSnapshot)

	sc.Step(`^the operator increases the counter by (\d+)$`, w.increaseBy)
	sc.Step(`^the operator increases the counter by (\d+) on behalf of a parent$`, w.increaseOnBehalf)
	sc.Step(`^the operator triggers a hard failure$`, w.triggerHardFailure)
	sc.Step(`^an unhandled command is dispatched$`, w.unhandledDispatched)
	sc.Step(`^a command with no command book is dispatched$`, w.commandNoBook)
	sc.Step(`^a command with an empty command book is dispatched$`, w.commandEmptyBook)
	sc.Step(`^a command whose page carries no payload is dispatched$`, w.commandNoPayload)
	sc.Step(`^a Reserve command is rejected$`, w.reserveRejected)
	sc.Step(`^an unregistered command is rejected$`, w.unregisteredRejected)

	sc.Step(`^(\d+) increases are recorded, starting at sequence (\d+)$`, w.recorded)
	sc.Step(`^(\d+) increases are recorded, continuing from sequence (\d+)$`, w.recorded)
	sc.Step(`^the command is rejected as ([A-Z_]+)$`, w.failsWith)
	sc.Step(`^the command fails with ([A-Z_]+)$`, w.failsWith)
	sc.Step(`^no events are recorded$`, w.noEventsRecorded)
	sc.Step(`^the recorded events carry the parent linkage$`, w.eventsCarryParentLinkage)
	sc.Step(`^the compensations run first then second$`, w.compensationsFirstThenSecond)
	sc.Step(`^no compensation is recorded$`, w.noCompensation)
	sc.Step(`^the handler saw (no )?prior history, at next sequence (\d+)$`, w.handlerSawHistory)
	sc.Step(`^the handler saw a counter of (\d+), at next sequence (\d+)$`, w.handlerSawCounter)
}
