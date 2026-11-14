//! Web builds (wasm32 with the `window` feature): the game draws into a
//! canvas on the page, panics show in the browser console, and asset files
//! are fetched over HTTP before setup runs.

pub(crate) mod ime;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use winit::platform::web::WindowAttributesExtWebSys;
use winit::window::WindowAttributes;

/// The id of the `<canvas>` the game draws into, when the page has one.
/// Without it, the engine adds a canvas at the end of the page.
pub(crate) const CANVAS_ID: &str = "ponko";

/// The list of asset files to fetch, relative to the page: a JSON array of
/// names under `assets/`, such as `["crab.png", "fonts/Title.ttf"]`.
const INDEX: &str = "assets/index.json";

/// The value of `name` in the page's query string (`?name=value&...`):
/// `Some("")` when it's there without a value, `None` when it isn't there.
pub(crate) fn query_param(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    crate::query::value(&search, name)
}

/// Sends panic messages to the browser console (they're lost otherwise).
pub(crate) fn show_panics_in_console() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&info.to_string()));
    }));
}

/// Draw into `<canvas id="ponko">`, else a new canvas added to the page.
pub(crate) fn canvas_attributes(attributes: WindowAttributes) -> WindowAttributes {
    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(CANVAS_ID))
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
    match canvas {
        Some(canvas) => attributes.with_canvas(Some(canvas)),
        None => attributes.with_append(true),
    }
}

/// Fetches every file listed in `assets/index.json`, all at once, so that
/// `load_texture` and friends find them. A page without the list has no
/// assets, which is fine for games that make their textures in code.
pub(crate) async fn fetch_assets() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let names: Vec<String> = match bytes(window.fetch_with_str(INDEX)).await {
        Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|e| {
            log_error(&format!("ponko: {INDEX} isn't a JSON list of file names: {e}"));
            Vec::new()
        }),
        Err(_) => return,
    };
    // Start every request before waiting for any.
    let requests: Vec<_> = names
        .into_iter()
        .map(|name| {
            let request = window.fetch_with_str(&format!("assets/{name}"));
            (name, request)
        })
        .collect();
    for (name, request) in requests {
        match bytes(request).await {
            Ok(data) => crate::assets::insert_web_file(name, data),
            Err(e) => log_error(&format!("ponko: could not fetch assets/{name}: {e}")),
        }
    }
}

/// Waits for a `fetch` and returns the body.
async fn bytes(request: js_sys::Promise) -> Result<Vec<u8>, String> {
    let response: web_sys::Response = JsFuture::from(request)
        .await
        .map_err(describe)?
        .dyn_into()
        .map_err(describe)?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let body = JsFuture::from(response.array_buffer().map_err(describe)?)
        .await
        .map_err(describe)?;
    Ok(js_sys::Uint8Array::new(&body).to_vec())
}

fn describe(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

fn log_error(message: &str) {
    web_sys::console::error_1(&JsValue::from_str(message));
}
