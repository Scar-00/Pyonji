use std::{collections::HashMap, io::Write as _};

use crate::{PushError, PyTheme as _, Pyonji};
use anyhow::Context as _;
use async_compat::CompatExt;
use gpui::{prelude::FluentBuilder as _, *};
use gpui_base::{Disableable, ScrollbarAxis, StyledExt as _, h_flex, v_flex};
use gpui_component::{
    Icon, IconName, Sizable, WindowExt, badge::Badge, button::{Button, ButtonVariants}, input::{Input, InputEvent, InputState}, label::Label, scroll::ScrollableElement, spinner::Spinner, text::TextView
};
use reqwest::Client;
use self_update::{Release, backends::github};
use smol::stream::StreamExt as _;
use crate::{Next, Prev, Submit};

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some(ReleasesView::CONTEXT)),
        KeyBinding::new("up", Prev, Some(ReleasesView::CONTEXT)),
        KeyBinding::new("enter", Submit, Some(ReleasesView::CONTEXT)),
    ]);
}

pub struct ReleasesView {
    _pyonji: WeakEntity<Pyonji>,

    releases: Option<Vec<Release>>,

    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    selected: Option<usize>,
    search_input: Entity<InputState>,
}

impl ReleasesView {
    pub const CONTEXT: &str = "ReleasesView";

    pub fn new(pyonji: &WeakEntity<Pyonji>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::fetch(window, cx);

        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search releases…")
                .clean_on_escape()
        });

        cx.subscribe(&search_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.selected = None;
                cx.notify();
            }
        })
        .detach();

        Self {
            _pyonji: pyonji.clone(),

            releases: None,

            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            selected: None,
            search_input,
        }
    }

    fn fetch(window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async |this, cx| -> Result<()> {
            let list = github::ReleaseList::configure()
                .repo_owner("Scar-00")
                .repo_name("Pyonji")
                .build().inspect_err(|e| {
                    _ = cx.update(|window, cx| {
                        window.dispatch_action(Box::new(PushError::new(format!("failed to build release list: {e}"))), cx);
                    });
                })?;
            let releases = list.fetch_async().compat().await.inspect_err(|e| {
                _ = cx.update(|window, cx| {
                    window.dispatch_action(Box::new(PushError::new(format!("failed to fetch release list: {e}"))), cx);
                });
            })?;
            _ = this.update(cx, |this, cx| {
                this.releases = Some(releases.into_vec());
                cx.notify();
            });
            Ok(())
        })
        .detach();
    }

    fn filtered(&self, cx: &App) -> Vec<usize> {
        let query = self.search_input.read(cx).value().to_lowercase();
        let Some(releases) = &self.releases else {
            return Vec::new();
        };

        releases
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                query.is_empty()
                    || r.version().to_lowercase().contains(&query)
                    || r.name().to_lowercase().contains(&query)
                    || r.body()
                        .as_deref()
                        .is_some_and(|b| b.to_lowercase().contains(&query))
            })
            .map(|(ix, _)| ix)
            .collect()
    }

    fn select(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.selected = ix;
        if let Some(ix) = ix {
            self.scroll_handle.scroll_to_item(ix);
        }
        cx.notify();
    }

    fn on_select_next(
        &mut self,
        _: &Next,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = self.filtered(cx);
        if visible.is_empty() {
            return;
        }
        let next = match self.selected.and_then(|s| {
            visible.iter().position(|&ix| ix == s)
        }) {
            Some(pos) => visible[(pos + 1).min(visible.len() - 1)],
            None => visible[0],
        };
        self.select(Some(next), cx);
    }

    fn on_select_prev(
        &mut self,
        _: &Prev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = self.filtered(cx);
        if visible.is_empty() {
            return;
        }
        let prev = match self.selected.and_then(|s| {
            visible.iter().position(|&ix| ix == s)
        }) {
            Some(pos) => visible[pos.saturating_sub(1)],
            None => visible[visible.len() - 1],
        };
        self.select(Some(prev), cx);
    }

    fn on_confirm(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected else { return };
        let Some(release) = self
            .releases
            .as_ref()
            .and_then(|r| r.get(ix))
            .cloned()
        else {
            return;
        };

        let Some(asset) = release.asset_for(self_update::get_target(), None) else {
            return;
        };

        let url = asset.download_url().to_string();

        cx.spawn_in(window, async move |this, cx| {
            if let Err(e) = Self::download_self(this, url, cx).await {
                _ = cx.update(|window, cx| {
                    window.dispatch_action(Box::new(PushError::new(e)), cx);
                });
            }
        })
        .detach();
    }

    async fn download_self(_: WeakEntity<Self>, url: String, cx: &mut AsyncWindowContext) -> Result<()> {
        let client = Client::new();
        let res = client
            .get(url)
            .header(reqwest::header::USER_AGENT, "Pyonji")
            .send()
            .compat()
            .await?;
        let body = res.json::<AssetResult>().compat().await?;
        let res = client
            .get(body.browser_download_url)
            .header(reqwest::header::USER_AGENT, "Pyonji")
            .send()
            .compat()
            .await?;
        let header = res
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .context("no content-length in download")?;
        let length = header.to_str().map(str::parse::<usize>)??;
        let mut stream = res.bytes_stream();
        let mut file = tempfile::NamedTempFile::new()?;
        let mut downloaded = 0;
        while let Some(chunk) = stream.next().compat().await {
            let chunk = chunk?;
            file.write_all(&chunk)?;
            downloaded += chunk.len();
            let progress = (downloaded as f64 / length as f64) * 100.0;
            println!("progress = {progress}");
        }
        let path = file.path();
        self_replace::self_replace(path)?;
        _ = cx.update(|_, cx| {
            cx.restart();
        });
        Ok(())
    }

    // ---------------------------------------------------------------- render

    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().unselected_border)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Label::new("Releases").font_semibold())
                    )
                    .child(
                        Button::new("reload")
                            .ghost()
                            .small()
                            //.icon(IconName::)
                            .tooltip("Reload releases")
                            .on_click(cx.listener(|_, _, window, cx| {
                                Self::fetch(window, cx);
                            })),
                    ),
            )
            .child(
                Input::new(&self.search_input)
                    .appearance(false)
                    .prefix(Icon::new(IconName::Search).small()),
            )
    }

    fn render_release(
        &self,
        ix: usize,
        release: &Release,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected == Some(ix);
        let is_latest = ix == 0;
        let theme = cx.theme();

        let summary = release
            .body()
            .as_deref()
            .and_then(|b| b.lines().find(|l| !l.trim().is_empty()))
            .map(|l| l.trim().to_string());

        div()
            .id(("release", ix))
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |this| this.bg(theme.selected.opacity(0.15)))
            .when(!selected, |this| this.hover(|s| s.bg(theme.selected.opacity(0.15))))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select(Some(ix), cx);
            }))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Label::new(format!("v{}", release.version()))
                                            .font_semibold(),
                                    )
                                    .when(is_latest, |this| {
                                        this.child(
                                            div()
                                                .px_1p5()
                                                .rounded_sm()
                                                .text_xs()
                                                .bg(theme.surface)
                                                .text_color(theme.text)
                                                .child("latest"),
                                        )
                                    }),
                            )
                            .child(
                                Label::new(release.date())
                                    .text_xs()
                                    .text_color(theme.text),
                            ),
                    )
                    .when(release.name() != release.version(), |this| {
                        this.child(
                            Label::new(release.name())
                                .text_sm()
                                .text_color(theme.text),
                        )
                    })
                    .when_some(summary, |this, s| {
                        this.child(
                            TextView::markdown(("summary", ix), s)
                        )
                    })
                    .child(
                        Label::new(format!(
                            "{} asset{}",
                            release.assets().len(),
                            if release.assets().len() == 1 { "" } else { "s" }
                        ))
                        .text_xs()
                        .text_color(theme.text),
                    ),
            )
    }

    fn render_body(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let Some(releases) = &self.releases else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Spinner::new())
                .child(
                    Label::new("Loading releases…")
                        .text_color(theme.text),
                )
                .into_any_element();
        };

        let visible = self.filtered(cx);

        if visible.is_empty() {
            let msg = if releases.is_empty() {
                "No releases found"
            } else {
                "No releases match your search"
            };
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Inbox)
                        .size_8()
                        .text_color(theme.text),
                )
                .child(Label::new(msg).text_color(theme.text))
                .into_any_element();
        }

        v_flex()
            .id("releases-list")
            .flex_1()
            .min_h_0()
            .p_1()
            .gap_0p5()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .scrollbar(&self.scroll_handle, ScrollbarAxis::Vertical)
            .children(
                visible
                    .into_iter()
                    .map(|ix| self.render_release(ix, &releases[ix], cx)),
            )
            .into_any_element()
    }

    fn render_footer(&self, cx: &Context<Self>) -> impl IntoElement {
        let selected = self
            .selected
            .and_then(|ix| self.releases.as_ref()?.get(ix));

        h_flex()
            .p_3()
            .justify_between()
            .items_center()
            .border_t_1()
            .border_color(cx.theme().unselected_border)
            .child(
                Label::new(match selected {
                    Some(r) => format!("Selected: v{}", r.version()),
                    None => "Select a release".to_string(),
                })
                .text_sm()
            )
            .child(
                Button::new("install")
                    .primary()
                    .small()
                    .label("Install")
                    .disabled(selected.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_confirm(&Submit, window, cx)
                    })),
            )
    }
}

impl Focusable for ReleasesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ReleasesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("releases-view")
            .key_context(Self::CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_confirm))
            .size_full()
            .child(self.render_header(cx))
            .child(self.render_body(cx))
            .child(self.render_footer(cx))
    }
}

#[derive(Debug, serde::Deserialize)]
struct AssetResult {
    browser_download_url: String,
    #[serde(flatten)]
    _rest: HashMap<String, serde_json::Value>,
}
