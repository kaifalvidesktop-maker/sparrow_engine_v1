# Shell integration (Go + cgo)

1. Build the engine as a C library:
   - `cargo build --release --target x86_64-pc-windows-gnu` (MinGW, same toolchain as the Sparrow shell)
   - outputs `sparrow_engine.dll` (+ `libsparrow_engine.dll.a` import lib). Copy next to `sparrow.h`.
2. Threading: **one engine = one OS thread.** Call every function of a handle from the same worker goroutine
   (`runtime.LockOSThread()`), never from the UI thread: `load_url` blocks on the network.
3. Frame loop: `run_timers(16)` -> `take_navigation()` -> `render()` -> blit RGBA to the WebView/canvas.
4. Crash UI: any call returning `2` (or `is_crashed()==1`) -> show "Page crashed, reload?"; on click `sparrow_recover()` then `sparrow_load_url(url)`.

```go
package sparrow

/*
#cgo windows LDFLAGS: -L${SRCDIR}/lib -lsparrow_engine
#include <stdlib.h>
#include "sparrow.h"
*/
import "C"
import (
	"errors"
	"unsafe"
)

var ErrCrashed = errors.New("Page crashed, reload?")

type Engine struct{ h *C.SparrowHandle }

func New() *Engine  { return &Engine{h: C.sparrow_create()} }
func (e *Engine) Close() { C.sparrow_destroy(e.h) }

func (e *Engine) lastErr() error { return errors.New(C.GoString(C.sparrow_last_error(e.h))) }

func (e *Engine) Load(url string) error {
	cs := C.CString(url)
	defer C.free(unsafe.Pointer(cs))
	switch C.sparrow_load_url(e.h, cs) {
	case 0:
		return nil
	case 2:
		return ErrCrashed
	}
	return e.lastErr()
}

func (e *Engine) Resize(w, h uint32) { C.sparrow_set_viewport(e.h, C.uint32_t(w), C.uint32_t(h)) }

// Render returns a copy of the RGBA frame.
func (e *Engine) Render() ([]byte, int, int, error) {
	var w, h C.uint32_t
	p := C.sparrow_render(e.h, &w, &h)
	if p == nil {
		if C.sparrow_is_crashed(e.h) == 1 {
			return nil, 0, 0, ErrCrashed
		}
		return nil, 0, 0, e.lastErr()
	}
	return C.GoBytes(unsafe.Pointer(p), C.int(w*h*4)), int(w), int(h), nil
}

func (e *Engine) Tick(ms uint32) (nav string) {
	C.sparrow_run_timers(e.h, C.uint32_t(ms))
	if n := C.sparrow_take_navigation(e.h); n != nil {
		return C.GoString(n)
	}
	return ""
}

func (e *Engine) Recover() { C.sparrow_recover(e.h) }
