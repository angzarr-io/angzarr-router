//go:build ffirouter

package conformance

import (
	"bytes"
	"context"
	"crypto/sha1"
	"errors"
	"fmt"
	"testing"

	"github.com/cucumber/godog"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/anypb"

	. "github.com/angzarr-io/angzarr-router/bindings/go"
	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
	counter "github.com/angzarr-io/angzarr-router/bindings/go/gen/test/counter"
)

// TestContextConformance runs the shared context.feature suite (facts, replay,
// cover access, PM compensator commands, projector page context) against
// components built through the binding's hand-written APIs.
func TestContextConformance(t *testing.T) {
	suite := godog.TestSuite{
		ScenarioInitializer: initializeContextScenario,
		Options: &godog.Options{
			Format:   "pretty",
			Paths:    []string{"../../../conformance/features/context.feature"},
			TestingT: t,
			Strict:   true,
		},
	}
	if suite.Run() != 0 {
		t.Fatal("context conformance scenarios failed")
	}
}

// namespaceOID is the RFC 4122 OID namespace (6ba7b812-9dad-11d1-80b4-00c04fd430c8).
var namespaceOID = []byte{
	0x6b, 0xa7, 0xb8, 0x12, 0x9d, 0xad, 0x11, 0xd1,
	0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
}

// rootOf is the root bytes for a label: UUID v5 in the OID namespace.
func rootOf(label string) []byte {
	h := sha1.New()
	h.Write(namespaceOID)
	h.Write([]byte(label))
	u := h.Sum(nil)[:16]
	u[6] = (u[6] & 0x0f) | 0x50
	u[8] = (u[8] & 0x3f) | 0x80
	return u
}

// coverOf is a cover in domain with the root for label.
func coverOf(domain, label string) *pb.Cover {
	return &pb.Cover{Domain: domain, Root: &pb.UUID{Value: rootOf(label)}}
}

// increasedPage is one Increased event page, at seq when non-nil.
func increasedPage(seq *uint32) *pb.EventPage {
	page := &pb.EventPage{Payload: &pb.EventPage_Event{Event: increasedAny()}}
	if seq != nil {
		page.Header = sequenceHeader(*seq)
	}
	return page
}

func seqPtr(seq uint32) *uint32 { return &seq }

// ledgerAggregate is the "ledger" aggregate over CounterState: Increased folds
// count += 1; a snapshot loads CounterState; IncreaseBy records the handled
// cover and emits nothing; an Increased fact is annotated as a CounterState
// carrying the folded count.
func ledgerAggregate(seen *[]*pb.Cover) *AggregateDispatch[*counter.CounterState] {
	rebuilder := NewRebuilder(func() *counter.CounterState { return &counter.CounterState{} }).
		Apply(fqIncreased, func(state *counter.CounterState, _ *anypb.Any) error {
			state.Count++
			return nil
		}).
		WithSnapshot(func(state *counter.CounterState, payload *anypb.Any) error {
			return payload.UnmarshalTo(state)
		})
	return NewAggregateDispatch("Ledger", "ledger", rebuilder).
		OnCommand(fqIncreaseBy, func(_ *anypb.Any, _ *counter.CounterState, cctx CommandContext) (*pb.EventBook, error) {
			*seen = append(*seen, cctx.Cover)
			return nil, nil
		}).
		OnFact(fqIncreased, func(_ *anypb.Any, state *counter.CounterState) (*anypb.Any, error) {
			return Pack(&counter.CounterState{Count: state.Count})
		})
}

// reservingPM is the "reserving-pm" process manager (target "inventory") over
// CounterState: an Increased trigger from "counter" records the trigger cover
// and emits nothing; a rejected Reserve is compensated with a Release command
// to "inventory".
func reservingPM(seen *[]*pb.Cover) *ProcessManagerDispatch[*counter.CounterState] {
	rebuilder := NewRebuilder(func() *counter.CounterState { return &counter.CounterState{} })
	return NewProcessManagerDispatch("Reserving", "reserving-pm", rebuilder, "inventory").
		OnEventWithCover("counter", fqIncreased, func(_ *anypb.Any, _ *counter.CounterState, _ *Destinations, triggerCover *pb.Cover) (*pb.ProcessManagerHandleResponse, error) {
			*seen = append(*seen, triggerCover)
			return &pb.ProcessManagerHandleResponse{}, nil
		}).
		OnRejectedResponse(fqReserve, func(*pb.Notification, *pb.RejectionNotification, *counter.CounterState) (*pb.ProcessManagerHandleResponse, error) {
			release := reserveCommand()
			release.Pages[0].GetCommand().TypeUrl = typeURL("test.counter.Release")
			return &pb.ProcessManagerHandleResponse{Commands: []*pb.CommandBook{release}}, nil
		})
}

// trackedPage is a (root, sequence) pair a projector fold observed.
type trackedPage struct {
	root     []byte
	sequence uint32
}

// trackingProjector records every Increased fold's book root and page
// sequence.
func trackingProjector(seen *[]trackedPage) *ProjectorDispatch[noState] {
	return NewProjectorDispatch("Tracker", newNoState).
		OnEventWithContext(fqIncreased, func(_ noState, _ *anypb.Any, ctx PageContext) error {
			*seen = append(*seen, trackedPage{root: ctx.Cover.GetRoot().GetValue(), sequence: ctx.Sequence})
			return nil
		})
}

// factRequest is facts pages of test.counter.<fact> in "ledger" over prior
// Increased events at sequences 0..prior-1.
func factRequest(fact string, facts, prior uint32) *pb.FactRequest {
	factPages := make([]*pb.EventPage, facts)
	for i := range factPages {
		factPages[i] = &pb.EventPage{Payload: &pb.EventPage_Event{Event: &anypb.Any{TypeUrl: typeURL("test.counter." + fact)}}}
	}
	priorPages := make([]*pb.EventPage, prior)
	for i := range priorPages {
		priorPages[i] = increasedPage(seqPtr(uint32(i)))
	}
	return &pb.FactRequest{
		Facts:       &pb.EventBook{Cover: &pb.Cover{Domain: "ledger"}, Pages: factPages},
		PriorEvents: &pb.EventBook{Pages: priorPages, NextSequence: prior},
	}
}

// replayRequest is a snapshot of count at sequence 1, then events Increased
// events at sequences 2...
func replayRequest(count, events uint32) *pb.ReplayRequest {
	pages := make([]*pb.EventPage, events)
	for i := range pages {
		pages[i] = increasedPage(seqPtr(2 + uint32(i)))
	}
	return &pb.ReplayRequest{
		BaseSnapshot: &pb.Snapshot{
			Sequence: 1,
			State: &anypb.Any{
				TypeUrl: typeURL("test.counter.CounterState"),
				Value:   mustMarshal(&counter.CounterState{Count: count}),
			},
		},
		Events: pages,
	}
}

// ledgerCommand is an IncreaseBy command for the ledger root of label.
func ledgerCommand(label string) *pb.ContextualCommand {
	return &pb.ContextualCommand{Command: &pb.CommandBook{
		Cover: coverOf("ledger", label),
		Pages: []*pb.CommandPage{{Payload: &pb.CommandPage_Command{Command: &anypb.Any{
			TypeUrl: typeURL(fqIncreaseBy),
			Value:   mustMarshal(&counter.IncreaseBy{N: 1}),
		}}}},
	}}
}

// reservingTrigger is an Increased trigger from "counter" root label at seq.
func reservingTrigger(label string, seq uint32) *pb.ProcessManagerHandleRequest {
	return &pb.ProcessManagerHandleRequest{Trigger: &pb.EventBook{
		Cover: coverOf("counter", label),
		Pages: []*pb.EventPage{increasedPage(&seq)},
	}}
}

// reservingRejection is the rejection of a Reserve sent to targetDomain,
// delivered to the reserving PM's own domain at sequence seq.
func reservingRejection(targetDomain string, seq uint32) *pb.ProcessManagerHandleRequest {
	rejected := reserveCommand()
	rejected.Cover = &pb.Cover{Domain: targetDomain}
	notification := &pb.Notification{Payload: &anypb.Any{
		TypeUrl: typeURL("io.angzarr.v1.RejectionNotification"),
		Value:   mustMarshal(&pb.RejectionNotification{RejectedCommand: rejected}),
	}}
	return &pb.ProcessManagerHandleRequest{Trigger: &pb.EventBook{
		Cover: &pb.Cover{Domain: "reserving-pm"},
		Pages: []*pb.EventPage{{
			Header: sequenceHeader(seq),
			Payload: &pb.EventPage_Event{Event: &anypb.Any{
				TypeUrl: typeURL("io.angzarr.v1.Notification"),
				Value:   mustMarshal(notification),
			}},
		}},
	}}
}

// trackedBook is a book of Increased events of "counter" root label at
// sequences.
func trackedBook(label string, sequences ...uint32) *pb.EventBook {
	pages := make([]*pb.EventPage, len(sequences))
	for i, seq := range sequences {
		pages[i] = increasedPage(seqPtr(seq))
	}
	return &pb.EventBook{Cover: coverOf("counter", label), Pages: pages}
}

type contextWorld struct {
	router   *Router
	covers   []*pb.Cover
	pages    []trackedPage
	facts    *pb.EventBook
	replayed *pb.ReplayResponse
	pm       *pb.ProcessManagerHandleResponse
	err      error
}

func (w *contextWorld) reset() {
	if w.router != nil {
		w.router.Close()
	}
	*w = contextWorld{router: NewRouter()}
}

// --- Given ---

func (w *contextWorld) ledger() error {
	return RegisterAggregate(w.router, ledgerAggregate(&w.covers))
}

func (w *contextWorld) reserving() error {
	return RegisterProcessManager(w.router, reservingPM(&w.covers))
}

func (w *contextWorld) tracking() error {
	return RegisterProjector(w.router, trackingProjector(&w.pages))
}

// --- When ---

func (w *contextWorld) increasedFacts(facts, prior int) {
	w.facts, w.err = w.router.DispatchFact(factRequest("Increased", uint32(facts), uint32(prior)))
}

func (w *contextWorld) reserveFact() {
	w.facts, w.err = w.router.DispatchFact(factRequest("Reserve", 1, 0))
}

func (w *contextWorld) replay(count, events int) {
	w.replayed, w.err = w.router.DispatchReplay("ledger", replayRequest(uint32(count), uint32(events)))
}

func (w *contextWorld) ledgerCommand(label string) {
	_, w.err = w.router.Dispatch(ledgerCommand(label))
}

func (w *contextWorld) reservingTrigger(label string, seq int) {
	w.pm, w.err = w.router.DispatchProcessManager(reservingTrigger(label, uint32(seq)))
}

func (w *contextWorld) reservingRejection(domain string, seq int) {
	w.pm, w.err = w.router.DispatchProcessManager(reservingRejection(domain, uint32(seq)))
}

func (w *contextWorld) projected(label string, first, second int) {
	_, w.err = w.router.DispatchProjector(trackedBook(label, uint32(first), uint32(second)))
}

// --- Then ---

func (w *contextWorld) recorded() (*pb.EventBook, error) {
	if w.err != nil {
		return nil, fmt.Errorf("fact handling failed: %w", w.err)
	}
	if w.facts == nil {
		return nil, errors.New("no facts were recorded")
	}
	return w.facts, nil
}

func (w *contextWorld) annotated(facts, count int) error {
	book, err := w.recorded()
	if err != nil {
		return err
	}
	if len(book.GetPages()) != facts {
		return fmt.Errorf("recorded %d facts, want %d", len(book.GetPages()), facts)
	}
	for i, page := range book.GetPages() {
		event := page.GetEvent()
		if got := fqFromURL(event.GetTypeUrl()); got != "test.counter.CounterState" {
			return fmt.Errorf("fact %d recorded as %s, want test.counter.CounterState", i, got)
		}
		var state counter.CounterState
		if err := proto.Unmarshal(event.GetValue(), &state); err != nil {
			return fmt.Errorf("fact %d: decode CounterState: %w", i, err)
		}
		if state.Count != uint32(count) {
			return fmt.Errorf("fact %d annotated with count %d, want %d", i, state.Count, count)
		}
	}
	return nil
}

func (w *contextWorld) unchanged() error {
	book, err := w.recorded()
	if err != nil {
		return err
	}
	if len(book.GetPages()) != 1 {
		return fmt.Errorf("recorded %d facts, want 1", len(book.GetPages()))
	}
	if got := fqFromURL(book.GetPages()[0].GetEvent().GetTypeUrl()); got != fqReserve {
		return fmt.Errorf("fact recorded as %s, want %s", got, fqReserve)
	}
	return nil
}

func (w *contextWorld) replayedCount(count int) error {
	if w.err != nil {
		return fmt.Errorf("replay failed: %w", w.err)
	}
	var state counter.CounterState
	if err := w.replayed.GetState().UnmarshalTo(&state); err != nil {
		return fmt.Errorf("replayed state is not a CounterState: %w", err)
	}
	if state.Count != uint32(count) {
		return fmt.Errorf("replayed count %d, want %d", state.Count, count)
	}
	return nil
}

func (w *contextWorld) sawRoot(label string) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	if len(w.covers) != 1 {
		return fmt.Errorf("the handler ran %d times, want once", len(w.covers))
	}
	if got, want := w.covers[0].GetRoot().GetValue(), rootOf(label); !bytes.Equal(got, want) {
		return fmt.Errorf("handler saw root %x, want %x", got, want)
	}
	return nil
}

func (w *contextWorld) release(seq int) error {
	if w.err != nil {
		return fmt.Errorf("PM dispatch failed: %w", w.err)
	}
	commands := w.pm.GetCommands()
	if len(commands) != 1 {
		return fmt.Errorf("emitted %d commands, want 1", len(commands))
	}
	page := commands[0].GetPages()[0]
	if got := fqFromURL(page.GetCommand().GetTypeUrl()); got != "test.counter.Release" {
		return fmt.Errorf("emitted %s, want test.counter.Release", got)
	}
	deferred := page.GetHeader().GetAngzarrDeferred()
	if deferred == nil {
		return fmt.Errorf("the Release command is not deferred (header %v)", page.GetHeader())
	}
	if deferred.GetSourceSeq() != uint32(seq) {
		return fmt.Errorf("deferred from source sequence %d, want %d", deferred.GetSourceSeq(), seq)
	}
	return nil
}

func (w *contextWorld) projectorSaw(label string, first, second int) error {
	if w.err != nil {
		return fmt.Errorf("projection failed: %w", w.err)
	}
	root := rootOf(label)
	want := []trackedPage{{root, uint32(first)}, {root, uint32(second)}}
	if len(w.pages) != len(want) {
		return fmt.Errorf("projector folded %d pages, want %d", len(w.pages), len(want))
	}
	for i, got := range w.pages {
		if !bytes.Equal(got.root, want[i].root) || got.sequence != want[i].sequence {
			return fmt.Errorf("fold %d saw (%x, %d), want (%x, %d)", i, got.root, got.sequence, want[i].root, want[i].sequence)
		}
	}
	return nil
}

func initializeContextScenario(sc *godog.ScenarioContext) {
	w := &contextWorld{}
	sc.Before(func(ctx context.Context, _ *godog.Scenario) (context.Context, error) {
		w.reset()
		return ctx, nil
	})
	sc.After(func(ctx context.Context, _ *godog.Scenario, _ error) (context.Context, error) {
		w.router.Close()
		return ctx, nil
	})

	sc.Step(`^a ledger aggregate$`, w.ledger)
	sc.Step(`^a reserving process-manager$`, w.reserving)
	sc.Step(`^a tracking projector$`, w.tracking)
	sc.Step(`^(\d+) Increased facts are handled over (\d+) prior Increased events$`, w.increasedFacts)
	sc.Step(`^a Reserve fact is handled over no prior events$`, w.reserveFact)
	sc.Step(`^the ledger replays a snapshot of (\d+) then (\d+) Increased events$`, w.replay)
	sc.Step(`^an IncreaseBy command for ledger root "([^"]*)" is dispatched$`, w.ledgerCommand)
	sc.Step(`^an Increased trigger of counter root "([^"]*)" at sequence (\d+) is dispatched to the reserving process-manager$`, w.reservingTrigger)
	sc.Step(`^a rejection of Reserve sent to "([^"]*)" at sequence (\d+) is dispatched to the reserving process-manager$`, w.reservingRejection)
	sc.Step(`^Increased events of counter root "([^"]*)" at sequences (\d+) and (\d+) are projected$`, w.projected)
	sc.Step(`^(\d+) facts are recorded, each annotated with a count of (\d+)$`, w.annotated)
	sc.Step(`^the fact is recorded unchanged$`, w.unchanged)
	sc.Step(`^the replayed state has a count of (\d+)$`, w.replayedCount)
	sc.Step(`^the ledger handler saw root "([^"]*)"$`, w.sawRoot)
	sc.Step(`^the reserving process-manager saw trigger root "([^"]*)"$`, w.sawRoot)
	sc.Step(`^the reserving process-manager emits one Release command deferred from source sequence (\d+)$`, w.release)
	sc.Step(`^the projector saw root "([^"]*)" at sequences (\d+) and (\d+)$`, w.projectorSaw)
}
