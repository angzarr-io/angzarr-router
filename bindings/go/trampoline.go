//go:build ffirouter

package ffirouter

/*
// Declarations only: a file using //export may not define C functions in
// its preamble. The angzarr_buf typedef and angzarr_buf_alloc are
// re-declared here (a separate cgo translation unit from ffirouter.go);
// identical typedefs across units are ABI-compatible.
#include <stdint.h>
#include <stddef.h>
typedef struct { uint8_t* data; size_t len; } angzarr_buf;
uint8_t* angzarr_buf_alloc(size_t);
*/
import "C"

import (
	"fmt"
	"runtime/cgo"
	"unsafe"
)

// angzarrGoTrampoline is the single C-visible gateway the core calls for
// every host callback. It recovers the dispatch session from host_ctx,
// routes by callback_id to the registered invoker, and writes the
// invoker's response into out via the router's allocator (router-owned, so
// the router frees it). A Go panic is caught and surfaced as a coded
// failure — it must never unwind across the boundary into Rust.
//
//export angzarrGoTrampoline
func angzarrGoTrampoline(
	ctx unsafe.Pointer, id C.uint64_t,
	tu *C.uint8_t, tul C.size_t,
	p *C.uint8_t, pl C.size_t,
	a *C.uint8_t, al C.size_t,
	out *C.angzarr_buf,
) (status C.int32_t) {
	defer func() {
		if rec := recover(); rec != nil {
			b, code := errorStatus(fmt.Errorf("go callback panicked: %v", rec))
			writeBuf(out, b)
			status = C.int32_t(code)
		}
	}()

	s := cgo.Handle(uintptr(ctx)).Value().(*session)
	inv, ok := s.router.registry[uint64(id)]
	if !ok {
		b, code := errorStatus(&CodedError{
			Code:    codeUnhandledHandlerError,
			Message: fmt.Sprintf("no host callback registered for id %d", uint64(id)),
			Grpc:    GrpcInternal,
		})
		writeBuf(out, b)
		return C.int32_t(code)
	}

	respBytes, st := inv(s, cBytesToString(tu, tul), cBytes(p, pl), cBytes(a, al))
	writeBuf(out, respBytes)
	return C.int32_t(st)
}

// writeBuf copies Go bytes into a router-allocated out buffer (the host
// fills out via the router's allocator; the router consumes and frees it).
// An empty payload leaves out null/zero.
func writeBuf(out *C.angzarr_buf, b []byte) {
	if out == nil {
		return
	}
	if len(b) == 0 {
		out.data = nil
		out.len = 0
		return
	}
	ptr := C.angzarr_buf_alloc(C.size_t(len(b)))
	dst := unsafe.Slice((*byte)(unsafe.Pointer(ptr)), len(b))
	copy(dst, b)
	out.data = ptr
	out.len = C.size_t(len(b))
}

// cBytes copies a router-owned input buffer (valid only for this callback)
// into Go memory.
func cBytes(p *C.uint8_t, n C.size_t) []byte {
	if p == nil || n == 0 {
		return nil
	}
	return C.GoBytes(unsafe.Pointer(p), C.int(n))
}

// cBytesToString copies a router-owned type-url buffer into a Go string.
func cBytesToString(p *C.uint8_t, n C.size_t) string {
	if p == nil || n == 0 {
		return ""
	}
	return C.GoStringN((*C.char)(unsafe.Pointer(p)), C.int(n))
}
