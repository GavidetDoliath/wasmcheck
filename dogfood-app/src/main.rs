use dioxus::prelude::*;

const PAYLOAD: &str = include_str!("blob.json");

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut counter = use_signal(|| 0_u32);
    let parsed: serde_json::Value = serde_json::from_str(PAYLOAD).expect("static json");
    let items = parsed["items"].as_array().map(Vec::len).unwrap_or(0);

    rsx! {
        document::Title { "wasmcheck dogfood" }
        div {
            style: "display:flex;flex-direction:column;gap:16px;align-items:center;font-family:system-ui;padding:48px;",
            h1 { "wasmcheck dogfood app" }
            p { "A deliberately small but real Dioxus app so wasmcheck has something honest to measure." }
            p { "loaded {items} config items" }
            button {
                onclick: move |_| counter += 1,
                "clicked {counter} times"
            }
        }
    }
}