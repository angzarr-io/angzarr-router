//go:build ffirouter

package conformance

import (
	"context"
	"errors"
	"fmt"
	"testing"

	"github.com/cucumber/godog"
	"google.golang.org/protobuf/types/known/anypb"

	. "github.com/angzarr-io/angzarr-router/bindings/go"
	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// TestCompensationConformance runs the shared compensation.feature suite
// against aggregates built through the binding's hand-written aggregate API.
func TestCompensationConformance(t *testing.T) {
	suite := godog.TestSuite{
		ScenarioInitializer: initializeCompensationScenario,
		Options: &godog.Options{
			Format:   "pretty",
			Paths:    []string{"../../../conformance/features/compensation.feature"},
			TestingT: t,
			Strict:   true,
		},
	}
	if suite.Run() != 0 {
		t.Fatal("compensation conformance scenarios failed")
	}
}

// oneEvent is a business response carrying one header-less event page of
// test.counter.<name>.
func oneEvent(name string) *pb.BusinessResponse {
	return &pb.BusinessResponse{Result: &pb.BusinessResponse_Events{Events: &pb.EventBook{
		Pages: []*pb.EventPage{markerPage(name)},
	}}}
}

// paymentAggregate is the "payment" aggregate: one compensator per
// (compensates entry, emitted event name) pair, each recording the
// rejection's code and message in seen.
func paymentAggregate(entries [][2]string, seen *rejectionSink) *AggregateDispatch[noState] {
	d := NewAggregateDispatch("Payment", "payment", NewRebuilder(newNoState))
	for _, entry := range entries {
		event := entry[1]
		d.OnRejected(entry[0], func(_ *pb.Notification, r *pb.RejectionNotification, _ noState, _ CommandContext) (*pb.BusinessResponse, error) {
			seen.record(r)
			return oneEvent(event), nil
		})
	}
	return d
}

// inventoryAggregate is the "inventory" aggregate: it undoes AdjustStock with
// StockAdjustmentReverted and Reserve with StockReleased.
func inventoryAggregate() *AggregateDispatch[noState] {
	undo := func(event string) UndoThunk[noState] {
		return func(*pb.Notification, *pb.Compensate, noState, CommandContext) (*pb.BusinessResponse, error) {
			return oneEvent(event), nil
		}
	}
	return NewAggregateDispatch("Inventory", "inventory", NewRebuilder(newNoState)).
		OnUndo("test.counter.AdjustStock", undo("StockAdjustmentReverted")).
		OnUndo(fqReserve, undo("StockReleased"))
}

// rejectionSentTo is the rejection of a test.counter.<command> sent to
// targetDomain, delivered to the payment aggregate.
func rejectionSentTo(command, targetDomain string, nextSequence *uint32) *pb.ContextualCommand {
	return rejectionWith(command, targetDomain, nextSequence, "", "")
}

// rejectionWith is rejectionSentTo whose rejection carries code (its machine
// code) and message (its rejection_reason).
func rejectionWith(command, targetDomain string, nextSequence *uint32, code, message string) *pb.ContextualCommand {
	rejected := reserveCommand()
	rejected.Cover = &pb.Cover{Domain: targetDomain}
	rejected.Pages[0].GetCommand().TypeUrl = typeURL("test.counter." + command)
	rejection := &pb.RejectionNotification{
		RejectedCommand: rejected,
		RejectionReason: message,
		Code:            code,
	}
	return notificationCommand("payment", &anypb.Any{
		TypeUrl: typeURL("io.angzarr.v1.RejectionNotification"),
		Value:   mustMarshal(rejection),
	}, nextSequence)
}

type compensationWorld struct {
	router *Router
	seen   rejectionSink
	resp   *pb.BusinessResponse
	err    error
}

func (w *compensationWorld) reset() {
	if w.router != nil {
		w.router.Close()
	}
	w.router = NewRouter()
	w.seen = nil
	w.resp = nil
	w.err = nil
}

func (w *compensationWorld) register(d *AggregateDispatch[noState]) error {
	if err := RegisterAggregate(w.router, d); err != nil {
		return fmt.Errorf("register aggregate: %w", err)
	}
	return nil
}

// --- Given ---

func (w *compensationWorld) paymentUnqualified(event string) error {
	return w.register(paymentAggregate([][2]string{{fqReserve, event}}, &w.seen))
}

func (w *compensationWorld) paymentQualified(firstDomain, firstEvent, secondDomain, secondEvent string) error {
	return w.register(paymentAggregate([][2]string{
		{firstDomain + ":" + fqReserve, firstEvent},
		{secondDomain + ":" + fqReserve, secondEvent},
	}, &w.seen))
}

func (w *compensationWorld) inventory() error {
	return w.register(inventoryAggregate())
}

// --- When ---

func (w *compensationWorld) rejectionSentTo(command, domain string) {
	w.resp, w.err = w.router.Dispatch(rejectionSentTo(command, domain, nil))
}

func (w *compensationWorld) rejectionOverHistory(command, domain string, last int) {
	next := uint32(last + 1)
	w.resp, w.err = w.router.Dispatch(rejectionSentTo(command, domain, &next))
}

func (w *compensationWorld) rejectionWithCode(command, code, message string) {
	w.resp, w.err = w.router.Dispatch(rejectionWith(command, "inventory", nil, code, message))
}

func (w *compensationWorld) rejectionWithoutCode(command, message string) {
	w.resp, w.err = w.router.Dispatch(rejectionWith(command, "inventory", nil, "", message))
}

func (w *compensationWorld) compensate(command string) {
	w.resp, w.err = w.router.Dispatch(notificationCommand("inventory", compensatePayload("test.counter."+command), nil))
}

// --- Then ---

func (w *compensationWorld) pages() ([]*pb.EventPage, error) {
	if w.err != nil {
		return nil, fmt.Errorf("dispatch failed: %w", w.err)
	}
	return w.resp.GetEvents().GetPages(), nil
}

func (w *compensationWorld) emitsOne(event string) error {
	pages, err := w.pages()
	if err != nil {
		return err
	}
	if len(pages) != 1 {
		return fmt.Errorf("emitted %d events, want exactly 1", len(pages))
	}
	if got := fqFromURL(pages[0].GetEvent().GetTypeUrl()); got != "test.counter."+event {
		return fmt.Errorf("emitted %s, want test.counter.%s", got, event)
	}
	return nil
}

func (w *compensationWorld) emitsNothing() error {
	pages, err := w.pages()
	if err != nil {
		return err
	}
	if len(pages) != 0 {
		return fmt.Errorf("emitted %d events, want none", len(pages))
	}
	return nil
}

func (w *compensationWorld) takesSequence(seq int) error {
	pages, err := w.pages()
	if err != nil {
		return err
	}
	if len(pages) == 0 {
		return errors.New("no event was emitted")
	}
	header := pages[0].GetHeader()
	if _, ok := header.GetSequenceType().(*pb.PageHeader_Sequence); !ok {
		return fmt.Errorf("emitted event carries no explicit sequence: %v", header)
	}
	if got := header.GetSequence(); got != uint32(seq) {
		return fmt.Errorf("emitted event at sequence %d, want %d", got, seq)
	}
	return nil
}

func (w *compensationWorld) emitInOrder(first string, firstSeq int, second string, secondSeq int) error {
	pages, err := w.pages()
	if err != nil {
		return err
	}
	want := []string{
		fmt.Sprintf("test.counter.%s@%d", first, firstSeq),
		fmt.Sprintf("test.counter.%s@%d", second, secondSeq),
	}
	got := make([]string, len(pages))
	for i, page := range pages {
		header := page.GetHeader()
		if _, ok := header.GetSequenceType().(*pb.PageHeader_Sequence); !ok {
			return fmt.Errorf("event %d carries no explicit sequence: %v", i, header)
		}
		got[i] = fmt.Sprintf("%s@%d", fqFromURL(page.GetEvent().GetTypeUrl()), header.GetSequence())
	}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		return fmt.Errorf("emitted %v, want %v", got, want)
	}
	return nil
}

func (w *compensationWorld) sawCode(code, message string) error {
	if w.err != nil {
		return fmt.Errorf("dispatch failed: %w", w.err)
	}
	return w.seen.exactly(code, message)
}

func (w *compensationWorld) sawNoCode(message string) error {
	return w.sawCode("", message)
}

func (w *compensationWorld) failsUnimplemented(code string) error {
	var ce *CodedError
	if !errors.As(w.err, &ce) {
		return fmt.Errorf("expected coded error %s, got %v", code, w.err)
	}
	if ce.Code != code {
		return fmt.Errorf("code = %s, want %s", ce.Code, code)
	}
	if ce.Grpc != GrpcUnimplemented {
		return fmt.Errorf("gRPC code = %d, want UNIMPLEMENTED (%d)", ce.Grpc, GrpcUnimplemented)
	}
	return nil
}

func initializeCompensationScenario(sc *godog.ScenarioContext) {
	w := &compensationWorld{}
	sc.Before(func(ctx context.Context, _ *godog.Scenario) (context.Context, error) {
		w.reset()
		return ctx, nil
	})
	sc.After(func(ctx context.Context, _ *godog.Scenario, _ error) (context.Context, error) {
		w.router.Close()
		return ctx, nil
	})

	sc.Step(`^a payment aggregate compensating Reserve from any domain with (\w+)$`, w.paymentUnqualified)
	sc.Step(`^a second payment aggregate compensating Reserve from any domain with (\w+)$`, w.paymentUnqualified)
	sc.Step(`^a payment aggregate compensating Reserve from "([^"]*)" with (\w+) and from "([^"]*)" with (\w+)$`, w.paymentQualified)
	sc.Step(`^an inventory aggregate undoing AdjustStock with StockAdjustmentReverted and Reserve with StockReleased$`, w.inventory)
	sc.Step(`^a rejection of (\w+) sent to "([^"]*)" is dispatched to the payment aggregate$`, w.rejectionSentTo)
	sc.Step(`^a rejection of (\w+) sent to "([^"]*)" is dispatched to the payment aggregate over history ending at sequence (\d+)$`, w.rejectionOverHistory)
	sc.Step(`^a rejection of (\w+) with code "([^"]*)" and message "([^"]*)" is dispatched to the payment aggregate$`, w.rejectionWithCode)
	sc.Step(`^a rejection of (\w+) with no code and message "([^"]*)" is dispatched to the payment aggregate$`, w.rejectionWithoutCode)
	sc.Step(`^a Compensate for (\w+) is dispatched to the inventory aggregate$`, w.compensate)
	sc.Step(`^the aggregate emits one (\w+) event$`, w.emitsOne)
	sc.Step(`^the aggregate emits nothing$`, w.emitsNothing)
	sc.Step(`^the aggregates emit (\w+) at sequence (\d+) then (\w+) at sequence (\d+)$`, w.emitInOrder)
	sc.Step(`^the emitted event takes sequence (\d+)$`, w.takesSequence)
	sc.Step(`^the dispatch fails with ([A-Z_]+) as UNIMPLEMENTED$`, w.failsUnimplemented)
	sc.Step(`^the compensation handler saw code "([^"]*)" and message "([^"]*)"$`, w.sawCode)
	sc.Step(`^the compensation handler saw an empty code and message "([^"]*)"$`, w.sawNoCode)
}
