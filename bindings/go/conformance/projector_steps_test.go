//go:build ffirouter

package conformance

import (
	"context"
	"errors"
	"fmt"
	"testing"

	"github.com/cucumber/godog"

	. "github.com/angzarr-io/angzarr-router/bindings/go"
	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
	counter "github.com/angzarr-io/angzarr-router/bindings/go/gen/test/counter"
)

// TestProjectorConformance runs the shared projector.feature behavior suite
// against the Go binding via godog — the same feature the Rust cucumber-rs
// harness drives against the core. Only the step layer is new.
func TestProjectorConformance(t *testing.T) {
	suite := godog.TestSuite{
		ScenarioInitializer: initializeProjectorScenario,
		Options: &godog.Options{
			Format:   "pretty",
			Paths:    []string{"../../../conformance/features/projector.feature"},
			TestingT: t,
			Strict:   true,
		},
	}
	if suite.Run() != 0 {
		t.Fatal("projector conformance scenarios failed")
	}
}

// projectorWorld holds one scenario's state: a router with the projector
// fixture registered, and the dispatch outcome.
type projectorWorld struct {
	router *Router
	proj   *pb.Projection
	err    error
}

func (w *projectorWorld) reset() {
	if w.router != nil {
		w.router.Close()
	}
	w.router = NewRouter()
	w.proj = nil
	w.err = nil
	if err := counter.RegisterCounterProjector(w.router, counterProjector{}); err != nil {
		panic(fmt.Sprintf("register projector fixture: %v", err))
	}
}

func (w *projectorWorld) dispatch(book *pb.EventBook) {
	w.proj, w.err = w.router.DispatchProjector(book)
}

// deliveryBook is an EventBook of n Increased events whose cover carries domain.
func deliveryBook(domain string, n uint32) *pb.EventBook {
	pages := make([]*pb.EventPage, n)
	for i := range pages {
		pages[i] = &pb.EventPage{Payload: &pb.EventPage_Event{Event: increasedAny()}}
	}
	return &pb.EventBook{Cover: &pb.Cover{Domain: domain}, Pages: pages}
}

// --- When ---

func (w *projectorWorld) eventsDelivered(n int, domain string) {
	w.dispatch(deliveryBook(domain, uint32(n)))
}

func (w *projectorWorld) deliveryNoCover() {
	book := deliveryBook("counter", 1)
	book.Cover = nil
	w.dispatch(book)
}

// --- Then ---

func (w *projectorWorld) recordsCount(n int) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if w.proj.GetSequence() != uint32(n) {
		return fmt.Errorf("projection records %d, want %d", w.proj.GetSequence(), n)
	}
	return nil
}

func (w *projectorWorld) recordsNothing() error {
	return w.recordsCount(0)
}

func (w *projectorWorld) failsWith(code string) error {
	var ce *CodedError
	if !errors.As(w.err, &ce) {
		return fmt.Errorf("expected coded error %s, got %v", code, w.err)
	}
	if ce.Code != code {
		return fmt.Errorf("expected %s, got %s", code, ce.Code)
	}
	return nil
}

func initializeProjectorScenario(sc *godog.ScenarioContext) {
	w := &projectorWorld{}
	sc.Before(func(ctx context.Context, _ *godog.Scenario) (context.Context, error) {
		w.reset()
		return ctx, nil
	})
	sc.After(func(ctx context.Context, _ *godog.Scenario, _ error) (context.Context, error) {
		w.router.Close()
		return ctx, nil
	})

	sc.Step(`^a counter projection$`, func() {})
	sc.Step(`^(\d+) events are delivered in domain "([^"]*)"$`, w.eventsDelivered)
	sc.Step(`^a delivery arrives with no cover$`, w.deliveryNoCover)
	sc.Step(`^the projection records (\d+) events?$`, w.recordsCount)
	sc.Step(`^the projection records nothing$`, w.recordsNothing)
	sc.Step(`^the delivery fails with ([A-Z_]+)$`, w.failsWith)
}
