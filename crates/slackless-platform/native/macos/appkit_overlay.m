// Click-through transparent desktop drawing overlay (`NSWindowSharingNone`) and floating panel
// configuration for Huddle screen-share controls.

#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <QuartzCore/QuartzCore.h>
#import <dispatch/dispatch.h>
#import <math.h>
#import <objc/runtime.h>
#import <stdbool.h>
#import <stddef.h>
#import <stdint.h>
#import <stdlib.h>
#import <string.h>

typedef void (*SlacklessOverlayMouseCallback)(void *ctx, uint8_t kind, double nx, double ny);

typedef struct SlacklessOverlaySegment {
    double ax, ay;
    double bx, by;
    double cx, cy;
    double dx, dy;
    int32_t hue;
    float alpha;
    float width;
} SlacklessOverlaySegment;

typedef struct SlacklessOverlayCursor {
    double x, y;
    int32_t hue;
    float alpha;
} SlacklessOverlayCursor;

static void hsl_hue_to_rgb(int32_t hue_deg, CGFloat *r, CGFloat *g, CGFloat *b) {
    double h = fmod((double)hue_deg, 360.0);
    if (h < 0.0) h += 360.0;
    double s = 0.85, l = 0.56;
    double c = (1.0 - fabs(2.0 * l - 1.0)) * s;
    double x = c * (1.0 - fabs(fmod(h / 60.0, 2.0) - 1.0));
    double m = l - c / 2.0;
    double rr = 0, gg = 0, bb = 0;
    if (h < 60) { rr = c; gg = x; }
    else if (h < 120) { rr = x; gg = c; }
    else if (h < 180) { gg = c; bb = x; }
    else if (h < 240) { gg = x; bb = c; }
    else if (h < 300) { rr = x; bb = c; }
    else { rr = c; bb = x; }
    *r = (CGFloat)(rr + m);
    *g = (CGFloat)(gg + m);
    *b = (CGFloat)(bb + m);
}

@interface SlacklessDrawOverlayView : NSView {
@public
    SlacklessOverlaySegment *segments;
    size_t segment_count;
    SlacklessOverlayCursor *cursors;
    size_t cursor_count;
    bool pen_enabled;
    int32_t pen_hue;
    SlacklessOverlayMouseCallback mouse_cb;
    void *mouse_ctx;
    NSTrackingArea *tracking_area;
}
- (void)syncTrackingArea;
@end

@implementation SlacklessDrawOverlayView

- (BOOL)isFlipped {
    return YES;
}

- (BOOL)isOpaque {
    return NO;
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    (void)event;
    return pen_enabled ? YES : NO;
}

- (void)dealloc {
    free(segments);
    free(cursors);
}

- (void)syncTrackingArea {
    if (tracking_area) {
        [self removeTrackingArea:tracking_area];
        tracking_area = nil;
    }
    if (pen_enabled) {
        NSTrackingAreaOptions opts = NSTrackingMouseMoved | NSTrackingActiveAlways |
                                     NSTrackingInVisibleRect | NSTrackingCursorUpdate;
        tracking_area = [[NSTrackingArea alloc] initWithRect:self.bounds
                                                     options:opts
                                                       owner:self
                                                    userInfo:nil];
        [self addTrackingArea:tracking_area];
    }
    if (self.window) {
        [self.window invalidateCursorRectsForView:self];
    }
}

- (void)resetCursorRects {
    [super resetCursorRects];
    if (pen_enabled) {
        [self addCursorRect:self.bounds cursor:[NSCursor crosshairCursor]];
    }
}

- (void)cursorUpdate:(NSEvent *)event {
    (void)event;
    if (pen_enabled) {
        [[NSCursor crosshairCursor] set];
    }
}

- (void)emitMouseKind:(uint8_t)kind event:(NSEvent *)event {
    if (!pen_enabled || !mouse_cb) return;
    NSPoint p = [self convertPoint:event.locationInWindow fromView:nil];
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    if (w < 2.0 || h < 2.0) return;
    double nx = (double)(p.x / w);
    double ny = (double)(p.y / h);
    if (nx < 0.0) nx = 0.0;
    if (nx > 1.0) nx = 1.0;
    if (ny < 0.0) ny = 0.0;
    if (ny > 1.0) ny = 1.0;
    mouse_cb(mouse_ctx, kind, nx, ny);
}

- (void)mouseDown:(NSEvent *)event {
    [self emitMouseKind:0 event:event];
}

- (void)mouseDragged:(NSEvent *)event {
    [self emitMouseKind:1 event:event];
}

- (void)mouseUp:(NSEvent *)event {
    [self emitMouseKind:2 event:event];
}

- (void)mouseMoved:(NSEvent *)event {
    [self emitMouseKind:3 event:event];
}

- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    CGContextRef ctx = [[NSGraphicsContext currentContext] CGContext];
    if (!ctx) return;

    CGContextClearRect(ctx, self.bounds);
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    if (w < 2.0 || h < 2.0) return;

    if (pen_enabled) {
        CGFloat br = 0, bg = 0, bb = 0;
        hsl_hue_to_rgb(pen_hue, &br, &bg, &bb);
        CGContextSetLineWidth(ctx, 4.0);
        CGContextSetRGBStrokeColor(ctx, br, bg, bb, 0.55);
        CGContextStrokeRect(ctx, CGRectInset(self.bounds, 2.0, 2.0));
    }

    CGContextSetLineCap(ctx, kCGLineCapRound);
    CGContextSetLineJoin(ctx, kCGLineJoinRound);

    for (size_t i = 0; i < segment_count; i++) {
        SlacklessOverlaySegment s = segments[i];
        if (s.alpha <= 0.01f) continue;
        CGFloat r = 0, g = 0, b = 0;
        hsl_hue_to_rgb(s.hue, &r, &g, &b);
        CGFloat lw = (CGFloat)(s.width > 0.5f ? s.width : 5.0f);

        CGPoint p0 = CGPointMake((CGFloat)s.ax * w, (CGFloat)s.ay * h);
        CGPoint p1 = CGPointMake((CGFloat)s.bx * w, (CGFloat)s.by * h);
        CGPoint p2 = CGPointMake((CGFloat)s.cx * w, (CGFloat)s.cy * h);
        CGPoint p3 = CGPointMake((CGFloat)s.dx * w, (CGFloat)s.dy * h);

        CGContextBeginPath(ctx);
        CGContextMoveToPoint(ctx, p0.x, p0.y);
        CGContextAddCurveToPoint(ctx, p1.x, p1.y, p2.x, p2.y, p3.x, p3.y);
        CGContextSetLineWidth(ctx, lw + 3.0);
        CGContextSetRGBStrokeColor(ctx, 1.0, 1.0, 1.0, (CGFloat)s.alpha * 0.45);
        CGContextStrokePath(ctx);

        CGContextBeginPath(ctx);
        CGContextMoveToPoint(ctx, p0.x, p0.y);
        CGContextAddCurveToPoint(ctx, p1.x, p1.y, p2.x, p2.y, p3.x, p3.y);
        CGContextSetLineWidth(ctx, lw);
        CGContextSetRGBStrokeColor(ctx, r, g, b, (CGFloat)s.alpha);
        CGContextStrokePath(ctx);
    }

    for (size_t i = 0; i < cursor_count; i++) {
        SlacklessOverlayCursor c = cursors[i];
        if (c.alpha <= 0.01f) continue;
        CGFloat r = 0, g = 0, b = 0;
        hsl_hue_to_rgb(c.hue, &r, &g, &b);
        CGFloat cx = (CGFloat)c.x * w;
        CGFloat cy = (CGFloat)c.y * h;
        CGContextSetRGBFillColor(ctx, r, g, b, (CGFloat)c.alpha);
        CGContextFillEllipseInRect(ctx, CGRectMake(cx - 6.0, cy - 6.0, 12.0, 12.0));
        CGContextSetLineWidth(ctx, 2.0);
        CGContextSetRGBStrokeColor(ctx, 1.0, 1.0, 1.0, (CGFloat)c.alpha * 0.9);
        CGContextStrokeEllipseInRect(ctx, CGRectMake(cx - 6.0, cy - 6.0, 12.0, 12.0));
    }
}

@end

static NSPanel *g_draw_overlay_win = nil;
static SlacklessDrawOverlayView *g_draw_overlay_view = nil;
static id g_pen_esc_local_monitor = nil;
static id g_pen_esc_global_monitor = nil;

static void remove_pen_escape_monitors(void) {
    if (g_pen_esc_local_monitor) {
        [NSEvent removeMonitor:g_pen_esc_local_monitor];
        g_pen_esc_local_monitor = nil;
    }
    if (g_pen_esc_global_monitor) {
        [NSEvent removeMonitor:g_pen_esc_global_monitor];
        g_pen_esc_global_monitor = nil;
    }
}

static void disable_pen_mode_now(bool notify_escape) {
    remove_pen_escape_monitors();
    SlacklessOverlayMouseCallback cb = NULL;
    void *ctx = NULL;
    if (g_draw_overlay_view) {
        cb = g_draw_overlay_view->mouse_cb;
        ctx = g_draw_overlay_view->mouse_ctx;
        g_draw_overlay_view->pen_enabled = false;
        g_draw_overlay_view->mouse_cb = NULL;
        g_draw_overlay_view->mouse_ctx = NULL;
        [g_draw_overlay_view syncTrackingArea];
        [g_draw_overlay_view setNeedsDisplay:YES];
    }
    if (g_draw_overlay_win) {
        g_draw_overlay_win.ignoresMouseEvents = YES;
        if (!g_draw_overlay_view ||
            (g_draw_overlay_view->segment_count == 0 && g_draw_overlay_view->cursor_count == 0)) {
            [g_draw_overlay_win orderOut:nil];
        }
    }
    if (notify_escape && cb) {
        cb(ctx, 4, 0.0, 0.0);
    }
}

static void install_pen_escape_monitors(void) {
    remove_pen_escape_monitors();
    g_pen_esc_local_monitor = [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown
                                                                    handler:^NSEvent *(NSEvent *event) {
                                                                        if (event.keyCode == 53) {
                                                                            disable_pen_mode_now(true);
                                                                            return nil;
                                                                        }
                                                                        return event;
                                                                    }];
    g_pen_esc_global_monitor = [NSEvent addGlobalMonitorForEventsMatchingMask:NSEventMaskKeyDown
                                                                      handler:^(NSEvent *event) {
                                                                          if (event.keyCode == 53) {
                                                                              dispatch_async(dispatch_get_main_queue(), ^{
                                                                                  disable_pen_mode_now(true);
                                                                              });
                                                                          }
                                                                      }];
}

static void ensure_draw_overlay_window(NSRect frame) {
    if (!g_draw_overlay_win) {
        NSPanel *win = [[NSPanel alloc]
            initWithContentRect:frame
                      styleMask:(NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel)
                        backing:NSBackingStoreBuffered
                          defer:NO];
        win.opaque = NO;
        win.backgroundColor = [NSColor clearColor];
        win.hasShadow = NO;
        win.hidesOnDeactivate = NO;
        win.floatingPanel = YES;
        win.becomesKeyOnlyIfNeeded = YES;
        win.ignoresMouseEvents = YES;
        win.acceptsMouseMovedEvents = YES;
        win.level = NSFloatingWindowLevel;
        win.sharingType = NSWindowSharingNone;
        win.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                                 NSWindowCollectionBehaviorFullScreenAuxiliary |
                                 NSWindowCollectionBehaviorStationary |
                                 NSWindowCollectionBehaviorIgnoresCycle;
        SlacklessDrawOverlayView *view =
            [[SlacklessDrawOverlayView alloc] initWithFrame:NSMakeRect(0, 0, frame.size.width, frame.size.height)];
        view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        win.contentView = view;
        g_draw_overlay_win = win;
        g_draw_overlay_view = view;
    } else if (!NSEqualRects(g_draw_overlay_win.frame, frame)) {
        [g_draw_overlay_win setFrame:frame display:NO];
    }
}

static bool resolve_share_target_nsrect(bool is_monitor, uint32_t target_id, NSRect *out_rect) {
    CGRect primary = CGDisplayBounds(CGMainDisplayID());
    CGFloat primary_h = primary.size.height;
    CGRect cg_rect = CGRectZero;
    bool found = false;

    if (is_monitor) {
        cg_rect = CGDisplayBounds((CGDirectDisplayID)target_id);
        found = cg_rect.size.width > 10.0 && cg_rect.size.height > 10.0;
    } else {
        CFArrayRef list = CGWindowListCopyWindowInfo(
            kCGWindowListOptionIncludingWindow, (CGWindowID)target_id);
        if (list) {
            NSArray *arr = (__bridge_transfer NSArray *)list;
            if (arr.count > 0) {
                NSDictionary *info = arr[0];
                NSDictionary *b = info[(id)kCGWindowBounds];
                if (b && CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)b, &cg_rect)) {
                    found = cg_rect.size.width > 10.0 && cg_rect.size.height > 10.0;
                }
            }
        }
    }
    if (!found) return false;
    CGFloat ns_y = primary_h - cg_rect.origin.y - cg_rect.size.height;
    *out_rect = NSMakeRect(cg_rect.origin.x, ns_y, cg_rect.size.width, cg_rect.size.height);
    return true;
}

void slackless_draw_overlay_update(
    bool is_monitor,
    uint32_t target_id,
    const SlacklessOverlaySegment *segs,
    size_t seg_count,
    const SlacklessOverlayCursor *cur,
    size_t cur_count) {
    SlacklessOverlaySegment *segs_copy = NULL;
    if (segs && seg_count > 0 && seg_count <= SIZE_MAX / sizeof(SlacklessOverlaySegment)) {
        segs_copy = (SlacklessOverlaySegment *)malloc(seg_count * sizeof(SlacklessOverlaySegment));
        if (segs_copy) memcpy(segs_copy, segs, seg_count * sizeof(SlacklessOverlaySegment));
    }
    SlacklessOverlayCursor *cur_copy = NULL;
    if (cur && cur_count > 0 && cur_count <= SIZE_MAX / sizeof(SlacklessOverlayCursor)) {
        cur_copy = (SlacklessOverlayCursor *)malloc(cur_count * sizeof(SlacklessOverlayCursor));
        if (cur_copy) memcpy(cur_copy, cur, cur_count * sizeof(SlacklessOverlayCursor));
    }

    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            bool pen_on = g_draw_overlay_view && g_draw_overlay_view->pen_enabled;
            if ((!segs_copy || seg_count == 0) && (!cur_copy || cur_count == 0)) {
                free(segs_copy);
                free(cur_copy);
                if (g_draw_overlay_view) {
                    free(g_draw_overlay_view->segments);
                    g_draw_overlay_view->segments = NULL;
                    g_draw_overlay_view->segment_count = 0;
                    free(g_draw_overlay_view->cursors);
                    g_draw_overlay_view->cursors = NULL;
                    g_draw_overlay_view->cursor_count = 0;
                    [g_draw_overlay_view setNeedsDisplay:YES];
                }
                if (pen_on) {
                    // Keep tracking window moves while pen mode is active; if the shared window
                    // disappeared/minimized, exit pen mode immediately so clicks are never trapped.
                    NSRect frame = NSZeroRect;
                    if (resolve_share_target_nsrect(is_monitor, target_id, &frame)) {
                        ensure_draw_overlay_window(frame);
                    } else {
                        disable_pen_mode_now(true);
                    }
                } else if (g_draw_overlay_win) {
                    [g_draw_overlay_win orderOut:nil];
                }
                return;
            }
            NSRect frame = NSZeroRect;
            if (!resolve_share_target_nsrect(is_monitor, target_id, &frame)) {
                free(segs_copy);
                free(cur_copy);
                if (pen_on) {
                    disable_pen_mode_now(true);
                } else if (g_draw_overlay_win) {
                    g_draw_overlay_win.ignoresMouseEvents = YES;
                    [g_draw_overlay_win orderOut:nil];
                }
                return;
            }
            ensure_draw_overlay_window(frame);
            free(g_draw_overlay_view->segments);
            g_draw_overlay_view->segments = segs_copy;
            g_draw_overlay_view->segment_count = segs_copy ? seg_count : 0;
            free(g_draw_overlay_view->cursors);
            g_draw_overlay_view->cursors = cur_copy;
            g_draw_overlay_view->cursor_count = cur_copy ? cur_count : 0;
            [g_draw_overlay_view setNeedsDisplay:YES];
            if (!g_draw_overlay_win.isVisible) {
                [g_draw_overlay_win orderFrontRegardless];
            }
        }
    });
}

void slackless_draw_overlay_set_pen_mode(
    bool enabled,
    bool is_monitor,
    uint32_t target_id,
    int32_t hue,
    void *ctx,
    SlacklessOverlayMouseCallback cb) {
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (!enabled) {
                disable_pen_mode_now(false);
                return;
            }

            NSRect frame = NSZeroRect;
            if (!resolve_share_target_nsrect(is_monitor, target_id, &frame)) {
                // Never leave a stale click-intercepting pen overlay behind if the shared window
                // closed or minimized before pen mode could attach to it.
                disable_pen_mode_now(false);
                if (g_draw_overlay_win) {
                    [g_draw_overlay_win orderOut:nil];
                }
                return;
            }
            ensure_draw_overlay_window(frame);
            g_draw_overlay_view->pen_enabled = true;
            g_draw_overlay_view->pen_hue = hue;
            g_draw_overlay_view->mouse_cb = cb;
            g_draw_overlay_view->mouse_ctx = ctx;
            g_draw_overlay_win.ignoresMouseEvents = NO;
            install_pen_escape_monitors();
            [g_draw_overlay_view syncTrackingArea];
            [g_draw_overlay_view setNeedsDisplay:YES];
            [g_draw_overlay_win orderFrontRegardless];
        }
    });
}

void slackless_draw_overlay_hide(void) {
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            remove_pen_escape_monitors();
            if (g_draw_overlay_view) {
                g_draw_overlay_view->pen_enabled = false;
                g_draw_overlay_view->mouse_cb = NULL;
                g_draw_overlay_view->mouse_ctx = NULL;
                [g_draw_overlay_view syncTrackingArea];
                free(g_draw_overlay_view->segments);
                g_draw_overlay_view->segments = NULL;
                g_draw_overlay_view->segment_count = 0;
                free(g_draw_overlay_view->cursors);
                g_draw_overlay_view->cursors = NULL;
                g_draw_overlay_view->cursor_count = 0;
            }
            if (g_draw_overlay_win) {
                g_draw_overlay_win.ignoresMouseEvents = YES;
                [g_draw_overlay_win orderOut:nil];
            }
        }
    });
}

static bool floating_panel_intersects_any_screen(NSWindow *win) {
    if (!win) return true;
    NSRect f = win.frame;
    for (NSScreen *scr in [NSScreen screens]) {
        NSRect inter = NSIntersectionRect(f, scr.visibleFrame);
        if (inter.size.width >= 24.0 && inter.size.height >= 20.0) {
            return true;
        }
    }
    return false;
}

static void clamp_floating_panel_to_visible_frame(NSWindow *win, CGFloat bottomMargin) {
    if (!win) return;
    NSScreen *scr = win.screen ?: [NSScreen mainScreen];
    if (!scr) return;
    NSRect vf = scr.visibleFrame;
    NSRect f = win.frame;
    CGFloat minY = vf.origin.y + bottomMargin;
    CGFloat maxY = NSMaxY(vf) - f.size.height;
    CGFloat minX = vf.origin.x;
    CGFloat maxX = NSMaxX(vf) - f.size.width;
    if (maxY < minY) maxY = minY;
    if (maxX < minX) maxX = minX;

    CGFloat nx = f.origin.x;
    CGFloat ny = f.origin.y;
    if (ny < minY) ny = minY;
    if (ny > maxY) ny = maxY;
    if (nx < minX) nx = minX;
    if (nx > maxX) nx = maxX;

    if (fabs(nx - f.origin.x) > 0.5 || fabs(ny - f.origin.y) > 0.5) {
        [win setFrameOrigin:NSMakePoint(nx, ny)];
    }
}

static const char kSlacklessFloatingPanelObserversKey = 0;

void slackless_configure_floating_panel(void *ns_view_ptr) {
    if (!ns_view_ptr) return;
    NSView *view = (__bridge NSView *)ns_view_ptr;
    void (^apply)(void) = ^{
        @autoreleasepool {
            NSWindow *win = view.window;
            if (!win) return;
            win.level = NSFloatingWindowLevel + 1;
            win.hidesOnDeactivate = NO;
            win.sharingType = NSWindowSharingNone;
            win.movableByWindowBackground = YES;
            win.opaque = NO;
            win.backgroundColor = [NSColor clearColor];
            win.hasShadow = YES;
            win.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                                     NSWindowCollectionBehaviorFullScreenAuxiliary;

            clamp_floating_panel_to_visible_frame(win, 12.0);

            // Deduplicate observers: never re-register move/resize/close observers if this
            // window was already configured.
            if (objc_getAssociatedObject(win, &kSlacklessFloatingPanelObserversKey) != nil) {
                return;
            }

            NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
            __weak NSWindow *weakWin = win;
            // Allow dragging freely across multiple displays; only pull the panel back onscreen if
            // it ends up completely outside every display's visible frame (e.g. monitor unplugged).
            id moveObs = [nc addObserverForName:NSWindowDidMoveNotification
                                         object:win
                                          queue:[NSOperationQueue mainQueue]
                                     usingBlock:^(NSNotification *note) {
                                         (void)note;
                                         NSWindow *w = weakWin;
                                         if (w && !floating_panel_intersects_any_screen(w)) {
                                             clamp_floating_panel_to_visible_frame(w, 0.0);
                                         }
                                     }];
            id resizeObs = [nc addObserverForName:NSWindowDidResizeNotification
                                           object:win
                                            queue:[NSOperationQueue mainQueue]
                                       usingBlock:^(NSNotification *note) {
                                           (void)note;
                                           clamp_floating_panel_to_visible_frame(weakWin, 0.0);
                                       }];
            __block __weak id weakCloseObs = nil;
            id closeObs = [nc addObserverForName:NSWindowWillCloseNotification
                                          object:win
                                           queue:[NSOperationQueue mainQueue]
                                      usingBlock:^(NSNotification *note) {
                                          (void)note;
                                          NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
                                          [center removeObserver:moveObs];
                                          [center removeObserver:resizeObs];
                                          id strongClose = weakCloseObs;
                                          if (strongClose) {
                                              [center removeObserver:strongClose];
                                          }
                                          NSWindow *strongWin = weakWin;
                                          if (strongWin) {
                                              objc_setAssociatedObject(
                                                  strongWin,
                                                  &kSlacklessFloatingPanelObserversKey,
                                                  nil,
                                                  OBJC_ASSOCIATION_RETAIN_NONATOMIC);
                                          }
                                      }];
            weakCloseObs = closeObs;
            objc_setAssociatedObject(
                win,
                &kSlacklessFloatingPanelObserversKey,
                @[moveObs, resizeObs, closeObs],
                OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        }
    };
    if ([NSThread isMainThread]) {
        apply();
    } else {
        dispatch_async(dispatch_get_main_queue(), apply);
    }
}
