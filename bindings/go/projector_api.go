package ffirouter

import (
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// ProjectorEventThunk folds one delivered event into the rebuilding
// projection. Binding/generated thunks unmarshal to the typed event and call
// the typed business method.
type ProjectorEventThunk[P any] func(projection P, event *anypb.Any) error

// ProjectorFinishThunk packs the folded projection instance into the wire
// Projection. When absent, dispatch returns a default Projection (cover +
// projector name).
type ProjectorFinishThunk[P any] func(projection P, events *pb.EventBook) (*pb.Projection, error)

// ProjectorUnknownThunk observes the type URL of an event with no fold thunk.
type ProjectorUnknownThunk func(typeURL string)

// ProjectorDispatch is one projector component's registration: its name, the
// domains it consumes (empty = all), its fold handlers, an optional catch-all
// for unhandled types, and an optional finisher. The shape mirrors the core's
// so generated wiring (unit 6) targets it with minimal emitter changes.
type ProjectorDispatch[P any] struct {
	name    string
	factory func() P
	domains []string
	events  map[string]ProjectorEventThunk[P]
	unknown ProjectorUnknownThunk
	finish  ProjectorFinishThunk[P]
}

// NewProjectorDispatch starts a projector registration over a fresh-projection
// factory.
func NewProjectorDispatch[P any](name string, factory func() P) *ProjectorDispatch[P] {
	return &ProjectorDispatch[P]{
		name:    name,
		factory: factory,
		events:  make(map[string]ProjectorEventThunk[P]),
	}
}

// ForDomains restricts folding to books whose cover carries one of these
// domains. Unset (the default) consumes every domain.
func (d *ProjectorDispatch[P]) ForDomains(domains ...string) *ProjectorDispatch[P] {
	d.domains = domains
	return d
}

// OnEvent registers the fold thunk for a fully-qualified event type name.
func (d *ProjectorDispatch[P]) OnEvent(fullName string, thunk ProjectorEventThunk[P]) *ProjectorDispatch[P] {
	d.events[fullName] = thunk
	return d
}

// OnUnknown registers a catch-all for events with no fold thunk.
func (d *ProjectorDispatch[P]) OnUnknown(thunk ProjectorUnknownThunk) *ProjectorDispatch[P] {
	d.unknown = thunk
	return d
}

// Finish registers the finisher that packs the folded instance into the wire
// Projection.
func (d *ProjectorDispatch[P]) Finish(thunk ProjectorFinishThunk[P]) *ProjectorDispatch[P] {
	d.finish = thunk
	return d
}
