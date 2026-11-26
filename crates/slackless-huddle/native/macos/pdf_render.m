// Native macOS PDF page counting and CoreGraphics bitmap -> PNG rendering.
//
// Uses `CGPDFDocument` + `CGBitmapContext` + `ImageIO` (`CGImageDestination`) so PDFs render
// natively at crisp Retina resolution with zero third-party PDF libraries.

#import "common.h"
#import <ImageIO/ImageIO.h>
#import <math.h>

static CGPDFDocumentRef open_pdf_document(const char *path) {
    if (!path || path[0] == '\0') return NULL;
    NSString *nsPath = [NSString stringWithUTF8String:path];
    if (!nsPath || nsPath.length == 0) return NULL;
    BOOL isDir = NO;
    if (![[NSFileManager defaultManager] fileExistsAtPath:nsPath isDirectory:&isDir] || isDir) {
        return NULL;
    }
    NSURL *url = [NSURL fileURLWithPath:nsPath];
    if (!url) return NULL;
    CGPDFDocumentRef doc = CGPDFDocumentCreateWithURL((__bridge CFURLRef)url);
    if (!doc) return NULL;
    if (CGPDFDocumentIsEncrypted(doc) && !CGPDFDocumentIsUnlocked(doc)) {
        if (!CGPDFDocumentUnlockWithPassword(doc, "")) {
            CGPDFDocumentRelease(doc);
            return NULL;
        }
    }
    return doc;
}

uint32_t slackless_pdf_page_count(const char *path) {
    @autoreleasepool {
        CGPDFDocumentRef doc = open_pdf_document(path);
        if (!doc) return 0;
        size_t count = CGPDFDocumentGetNumberOfPages(doc);
        CGPDFDocumentRelease(doc);
        if (count > (size_t)UINT32_MAX) {
            return UINT32_MAX;
        }
        return (uint32_t)count;
    }
}

bool slackless_pdf_render_page_png(
    const char *path,
    uint32_t page_index,
    uint32_t max_w,
    uint32_t max_h,
    uint8_t **out_buf,
    size_t *out_len,
    uint32_t *out_w,
    uint32_t *out_h) {
    if (out_buf) *out_buf = NULL;
    if (out_len) *out_len = 0;
    if (out_w) *out_w = 0;
    if (out_h) *out_h = 0;
    if (!path || !out_buf || !out_len || !out_w || !out_h) return false;

    @autoreleasepool {
        CGPDFDocumentRef doc = open_pdf_document(path);
        if (!doc) return false;

        size_t count = CGPDFDocumentGetNumberOfPages(doc);
        if (count == 0 || (size_t)page_index >= count) {
            CGPDFDocumentRelease(doc);
            return false;
        }

        // CoreGraphics PDF pages are 1-indexed.
        CGPDFPageRef page = CGPDFDocumentGetPage(doc, (size_t)page_index + 1);
        if (!page) {
            CGPDFDocumentRelease(doc);
            return false;
        }

        CGPDFBox box_kind = kCGPDFCropBox;
        CGRect page_rect = CGPDFPageGetBoxRect(page, box_kind);
        if (CGRectIsEmpty(page_rect) || page_rect.size.width <= 0.0 || page_rect.size.height <= 0.0) {
            box_kind = kCGPDFMediaBox;
            page_rect = CGPDFPageGetBoxRect(page, box_kind);
        }
        if (CGRectIsEmpty(page_rect) || page_rect.size.width <= 0.0 || page_rect.size.height <= 0.0) {
            CGPDFDocumentRelease(doc);
            return false;
        }

        int rot = CGPDFPageGetRotationAngle(page);
        bool swapped = (abs(rot) % 180) == 90;
        double src_w = swapped ? page_rect.size.height : page_rect.size.width;
        double src_h = swapped ? page_rect.size.width : page_rect.size.height;
        if (!isfinite(src_w) || !isfinite(src_h) || src_w <= 0.0 || src_h <= 0.0) {
            CGPDFDocumentRelease(doc);
            return false;
        }

        double limit_w = (double)(max_w > 0 ? (max_w > 4096 ? 4096 : max_w) : 1600);
        double limit_h = (double)(max_h > 0 ? (max_h > 4096 ? 4096 : max_h) : 2200);
        double scale = fmin(limit_w / src_w, limit_h / src_h);
        if (!isfinite(scale) || scale <= 0.0) scale = 1.0;

        uint32_t target_w = (uint32_t)llround(src_w * scale);
        uint32_t target_h = (uint32_t)llround(src_h * scale);
        if (target_w < 1) target_w = 1;
        if (target_h < 1) target_h = 1;
        if (target_w > 4096) target_w = 4096;
        if (target_h > 4096) target_h = 4096;

        CGColorSpaceRef cs = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        if (!cs) cs = CGColorSpaceCreateDeviceRGB();
        if (!cs) {
            CGPDFDocumentRelease(doc);
            return false;
        }

        CGContextRef ctx = CGBitmapContextCreate(
            NULL,
            target_w,
            target_h,
            8,
            (size_t)target_w * 4,
            cs,
            (CGBitmapInfo)kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
        CGColorSpaceRelease(cs);
        if (!ctx) {
            CGPDFDocumentRelease(doc);
            return false;
        }

        CGRect target_rect = CGRectMake(0.0, 0.0, (CGFloat)target_w, (CGFloat)target_h);
        // Paint opaque white paper background behind transparent PDF pages.
        CGContextSetRGBFillColor(ctx, 1.0, 1.0, 1.0, 1.0);
        CGContextFillRect(ctx, target_rect);
        CGContextSetInterpolationQuality(ctx, kCGInterpolationHigh);
        CGContextSetShouldAntialias(ctx, true);
        CGContextSetShouldSmoothFonts(ctx, true);

        CGContextSaveGState(ctx);
        CGAffineTransform transform =
            CGPDFPageGetDrawingTransform(page, box_kind, target_rect, 0, true);
        CGContextConcatCTM(ctx, transform);
        CGContextDrawPDFPage(ctx, page);
        CGContextRestoreGState(ctx);

        CGImageRef image = CGBitmapContextCreateImage(ctx);
        CGContextRelease(ctx);
        CGPDFDocumentRelease(doc);
        if (!image) return false;

        CFMutableDataRef png_data = CFDataCreateMutable(kCFAllocatorDefault, 0);
        if (!png_data) {
            CGImageRelease(image);
            return false;
        }

        CGImageDestinationRef dest =
            CGImageDestinationCreateWithData(png_data, CFSTR("public.png"), 1, NULL);
        if (!dest) {
            CFRelease(png_data);
            CGImageRelease(image);
            return false;
        }
        CGImageDestinationAddImage(dest, image, NULL);
        bool finalized = CGImageDestinationFinalize(dest);
        CFRelease(dest);
        CGImageRelease(image);
        if (!finalized) {
            CFRelease(png_data);
            return false;
        }

        CFIndex len = CFDataGetLength(png_data);
        if (len <= 0) {
            CFRelease(png_data);
            return false;
        }

        uint8_t *copy = (uint8_t *)malloc((size_t)len);
        if (!copy) {
            CFRelease(png_data);
            return false;
        }
        memcpy(copy, CFDataGetBytePtr(png_data), (size_t)len);
        CFRelease(png_data);

        *out_buf = copy;
        *out_len = (size_t)len;
        *out_w = target_w;
        *out_h = target_h;
        return true;
    }
}
