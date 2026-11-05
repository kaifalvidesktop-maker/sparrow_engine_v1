#ifndef SPARROW_H
#define SPARROW_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct SparrowHandle SparrowHandle; /* opaque (Rust: ffi::Handle) */

/* Return codes: 0 ok, 1 error (sparrow_last_error), 2 page crashed -> show "Page crashed, reload?" then sparrow_recover */
SparrowHandle* sparrow_create(void);
void           sparrow_destroy(SparrowHandle*);

int            sparrow_load_url(SparrowHandle*, const char* url);
int            sparrow_load_html(SparrowHandle*, const char* html, const char* base_url);
int            sparrow_set_viewport(SparrowHandle*, uint32_t w, uint32_t h);
const uint8_t* sparrow_render(SparrowHandle*, uint32_t* out_w, uint32_t* out_h); /* RGBA, NULL on error; valid until next render */
const char*    sparrow_eval_js(SparrowHandle*, const char* code);                /* NULL on error */
float          sparrow_scroll_by(SparrowHandle*, float dy);                      /* new scroll_y, -1 on error */

const char*    sparrow_last_error(SparrowHandle*);
const char*    sparrow_title(SparrowHandle*);
const char*    sparrow_url(SparrowHandle*);
const char*    sparrow_take_logs(SparrowHandle*);        /* '\n' separated */
const char*    sparrow_take_navigation(SparrowHandle*);  /* NULL if none */
const char*    sparrow_hit_test(SparrowHandle*, float x, float y); /* link URL or NULL */
const char*    sparrow_find(SparrowHandle*, const char* query);    /* "x,y,w,h;x,y,w,h" */
const char*    sparrow_page_text(SparrowHandle*);

int            sparrow_fire_event(SparrowHandle*, const char* element_id, const char* event);
int            sparrow_run_timers(SparrowHandle*, uint32_t elapsed_ms);
int            sparrow_is_crashed(SparrowHandle*);
void           sparrow_recover(SparrowHandle*);

/* Strings returned are owned by the handle: copy them before the next call on the same handle. */
#ifdef __cplusplus
}
#endif
#endif
