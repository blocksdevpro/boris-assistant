//! `screen_capture`: let the model see the screen (privacy-sensitive, HITL).
//!
//! Captures the primary monitor or the active window with GDI on Windows,
//! downscales to at most [`MAX_IMAGE_EDGE`] px on the long side, and attaches
//! the PNG to the turn through [`ToolCallContext::attach_image`]. The loop
//! delivers it as a user-role image message after the tool batch (tool
//! messages are text-only on many providers). Every call needs the user's
//! yes: the tool is `Dangerous` + `requires_confirmation`. Other platforms
//! return a clear error.

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::tool::{Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk};
use crate::tool_context::ToolCallContext;

/// Long-side cap for images sent to the model (Anthropic's native limit;
/// OpenAI high detail tiles it efficiently).
pub const MAX_IMAGE_EDGE: u32 = 1568;

/// Captured and downscaled frame ready for the model.
pub(crate) struct EncodedImage {
    pub png_base64: String,
    pub width: u32,
    pub height: u32,
}

/// Downscale (never upscale) RGBA pixels to `max_edge` and encode PNG.
pub(crate) fn encode_png_scaled(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    max_edge: u32,
) -> Result<EncodedImage, String> {
    let image = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "captured pixel buffer has the wrong size".to_string())?;
    let image = image::DynamicImage::ImageRgba8(image);
    encode_dynamic_scaled(image, max_edge)
}

pub(crate) fn encode_dynamic_scaled(
    image: image::DynamicImage,
    max_edge: u32,
) -> Result<EncodedImage, String> {
    let (width, height) = (image.width(), image.height());
    let long = width.max(height).max(1);
    let image = if long > max_edge {
        let scale = f64::from(max_edge) / f64::from(long);
        let w = ((f64::from(width) * scale).round() as u32).max(1);
        let h = ((f64::from(height) * scale).round() as u32).max(1);
        image.resize_exact(w, h, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    // Screens are opaque; dropping alpha shrinks the PNG.
    let rgb = image::DynamicImage::ImageRgb8(image.to_rgb8());
    let mut png = Vec::new();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| format!("PNG encode failed: {error}"))?;
    Ok(EncodedImage {
        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
        width: rgb.width(),
        height: rgb.height(),
    })
}

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Screen,
    Window,
}

/// Capture the primary monitor or the active window.
pub struct ScreenCaptureTool;

#[async_trait]
impl Tool for ScreenCaptureTool {
    fn name(&self) -> &str {
        "screen_capture"
    }

    fn description(&self) -> &str {
        "Take a screenshot so you can see the user's screen. target \"screen\" (default) \
         captures the primary monitor; \"window\" captures the active window. The user is \
         asked to approve every capture. The image arrives in the next message (downscaled \
         to 1568 px on the long side); describe only what the user asked about."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "enum": ["screen", "window"],
                    "description": "\"screen\" (primary monitor, default) or \"window\" (active window)"
                }
            }
        })
    }

    fn meta(&self) -> ToolMeta {
        // Privacy-sensitive: always confirm, even in a trusted session.
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .kind(ToolKind::System)
            .permissions(&[Permission::UiControl])
            .confirm(true)
            .read_only(true)
            .max_concurrency(1)
            .timeout(std::time::Duration::from_secs(15))
    }

    async fn execute(&self, ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let target = match args.get("target").and_then(Value::as_str) {
            None | Some("screen") | Some("") => Target::Screen,
            Some("window") => Target::Window,
            Some(other) => {
                return Err(ToolError::invalid_args(format!(
                    "target must be \"screen\" or \"window\", got {other:?}"
                )))
            }
        };
        if !ctx.can_attach_images() {
            return Err(ToolError::failed(
                "this host cannot deliver images to the model",
            ));
        }
        let captured = tokio::task::spawn_blocking(move || capture(target))
            .await
            .map_err(|error| ToolError::failed(format!("capture task failed: {error}")))?
            .map_err(ToolError::failed)?;
        let what = match target {
            Target::Screen => "primary monitor",
            Target::Window => "active window",
        };
        let caption = format!("{what}, {}x{} px", captured.width, captured.height);
        ctx.attach_image(self.name(), "image/png", captured.png_base64, caption.clone());
        Ok(format!(
            "Captured the {what} ({}x{} px). The image follows in the next message.",
            captured.width, captured.height
        ))
    }
}

#[cfg(not(windows))]
fn capture(_target: Target) -> Result<EncodedImage, String> {
    Err("screen capture is only available on Windows".into())
}

#[cfg(windows)]
fn capture(target: Target) -> Result<EncodedImage, String> {
    let (x, y, width, height) = match target {
        Target::Screen => gdi::primary_monitor_rect(),
        Target::Window => gdi::foreground_window_rect()?,
    };
    if width <= 0 || height <= 0 {
        return Err("nothing to capture (zero-sized area)".into());
    }
    let rgba = gdi::grab(x, y, width, height)?;
    encode_png_scaled(rgba, width as u32, height as u32, MAX_IMAGE_EDGE)
}

#[cfg(windows)]
mod gdi {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN,
    };

    pub(super) fn primary_monitor_rect() -> (i32, i32, i32, i32) {
        // SAFETY: plain metric queries with constant indices.
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        (0, 0, w, h)
    }

    pub(super) fn foreground_window_rect() -> Result<(i32, i32, i32, i32), String> {
        // SAFETY: GetForegroundWindow has no preconditions; GetWindowRect
        // writes into a valid, owned RECT.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() {
                return Err("no active window".into());
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetWindowRect(hwnd, &mut rect) == 0 {
                return Err("could not read the active window bounds".into());
            }
            Ok((
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ))
        }
    }

    /// Copy a screen rectangle into top-down RGBA bytes.
    pub(super) fn grab(x: i32, y: i32, width: i32, height: i32) -> Result<Vec<u8>, String> {
        // SAFETY: every GDI handle created here is checked, used on this
        // thread only, and released on all paths before returning. The pixel
        // buffer is sized exactly for a 32-bpp top-down DIB of width×height.
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return Err("could not open the screen device context".into());
            }
            let memory = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            if memory.is_null() || bitmap.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                if !memory.is_null() {
                    DeleteDC(memory);
                }
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err("could not allocate a capture bitmap".into());
            }
            let previous = SelectObject(memory, bitmap);
            let copied = BitBlt(
                memory,
                0,
                0,
                width,
                height,
                screen,
                x,
                y,
                SRCCOPY | CAPTUREBLT,
            );
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative height = top-down rows.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            let rows = if copied != 0 {
                GetDIBits(
                    memory,
                    bitmap,
                    0,
                    height as u32,
                    pixels.as_mut_ptr().cast(),
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } else {
                0
            };
            SelectObject(memory, previous);
            DeleteObject(bitmap);
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut(), screen);
            if rows == 0 {
                return Err("screen copy failed".into());
            }
            // GDI returns BGRA; the encoder wants RGBA.
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                pixel[3] = 255;
            }
            Ok(pixels)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_frames_are_downscaled_to_the_long_edge_and_small_ones_kept() {
        let big = encode_png_scaled(vec![200; 3200 * 1800 * 4], 3200, 1800, MAX_IMAGE_EDGE)
            .unwrap();
        assert_eq!((big.width, big.height), (1568, 882));
        assert!(!big.png_base64.is_empty());
        let small = encode_png_scaled(vec![10; 640 * 480 * 4], 640, 480, MAX_IMAGE_EDGE).unwrap();
        assert_eq!((small.width, small.height), (640, 480));
        assert!(encode_png_scaled(vec![0; 10], 640, 480, MAX_IMAGE_EDGE).is_err());
    }

    #[test]
    fn capture_requires_confirmation_and_rejects_bad_targets() {
        let meta = ScreenCaptureTool.meta();
        assert!(meta.requires_confirmation);
        assert_eq!(meta.risk, ToolRisk::Dangerous);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = runtime
            .block_on(ScreenCaptureTool.execute(
                &ToolCallContext::new("c"),
                json!({"target": "desktop"}),
            ))
            .unwrap_err();
        assert!(err.to_string().contains("target"));
        // Without an image sink the tool refuses instead of capturing.
        let err = runtime
            .block_on(ScreenCaptureTool.execute(&ToolCallContext::new("c"), json!({})))
            .unwrap_err();
        assert!(err.to_string().contains("cannot deliver images"));
    }
}
