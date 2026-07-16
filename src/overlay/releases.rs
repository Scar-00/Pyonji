use anyhow::{Context, Result};
use async_compat::CompatExt;
use futures::StreamExt;
use ratatui::{prelude::*, text::ToText, widgets::*};
use reqwest::Client;
use self_update::{backends::github, Release};
use std::{cell::RefCell, collections::HashMap, io::Write, rc::Rc, sync::Arc};
use winit::{keyboard::KeyCode, window::Window};

use crate::App;

#[derive(Default)]
pub struct ReleasesView;

pub enum DownloadState {
    ReleaseDownload { progress: u8 },
    ReleaseDownloadResult(Result<()>),
}

#[derive(Clone)]
pub struct ReleasesState(Rc<RefCell<ReleasesStateInner>>);

impl ReleasesState {
    fn update<R>(&self, f: impl FnOnce(&mut ReleasesStateInner) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }
}

struct ReleasesStateInner {
    releases: Vec<Release>,

    download_state: Option<DownloadState>,

    list_state: ListState,
}

impl ReleasesState {
    pub fn new(app: &App) -> Self {
        let this = Self(Rc::new(RefCell::new(ReleasesStateInner {
            releases: vec![],
            download_state: None,
            list_state: ListState::default(),
        })));
        app.local_executer
            .spawn({
                let this = this.clone();
                async move || {
                    let Ok(release_list) = github::ReleaseList::configure()
                        .repo_owner("Scar-00")
                        .repo_name("Pyonji")
                        .build()
                    else {
                        return;
                    };
                    let Ok(releases) = release_list.fetch_async().compat().await else {
                        return;
                    };
                    this.update(|this| {
                        this.releases = releases.into_vec();
                        this.list_state.select_first();
                    });
                }
            })
            .detach();

        this
    }

    pub fn handle_events(&mut self, app: &mut App, code: KeyCode) {
        let mut this = self.0.borrow_mut();
        match code {
            KeyCode::ArrowDown => {
                this.list_state.select_next();
            }
            KeyCode::ArrowUp => {
                this.list_state.select_previous();
            }
            KeyCode::Enter if this.download_state.is_none() => {
                let Some(selected) = this.list_state.selected() else {
                    return;
                };
                let Some(asset) =
                    this.releases[selected].asset_for(self_update::get_target(), None)
                else {
                    return;
                };
                let url = asset.download_url().to_string();
                let this = self.0.clone();
                let window = app.window.clone();
                app.local_executer
                    .spawn({
                        async move || {
                            let url = url.clone();
                            let this = this.clone();
                            let window = window.clone();
                            let Some(window) = window else {
                                return;
                            };
                            let res = Self::download_self(this.clone(), window.clone(), url).await;
                            this.borrow_mut().download_state =
                                Some(DownloadState::ReleaseDownloadResult(res));
                            window.request_redraw();
                        }
                    })
                    .detach();
            }
            KeyCode::Enter
                if let Some(DownloadState::ReleaseDownloadResult(Ok(_))) = this.download_state => {}
            KeyCode::Escape
                if let Some(DownloadState::ReleaseDownloadResult(Ok(_))) = this.download_state =>
            {
                this.download_state = None;
                app.request_redraw();
            }
            _ => {}
        }
    }

    async fn download_self(
        this: Rc<RefCell<ReleasesStateInner>>,
        window: Arc<Window>,
        url: String,
    ) -> Result<()> {
        this.borrow_mut().download_state = Some(DownloadState::ReleaseDownload { progress: 0 });
        window.request_redraw();
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
            let progrss = (downloaded as f64 / length as f64) * 100.0;
            this.borrow_mut().download_state = Some(DownloadState::ReleaseDownload {
                progress: progrss as u8,
            });
            window.request_redraw();
        }
        let path = file.path();
        self_replace::self_replace(path)?;
        Ok(())
    }
}

impl StatefulWidget for ReleasesView {
    type State = ReleasesState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let mut state = state.0.borrow_mut();
        match state.download_state {
            Some(DownloadState::ReleaseDownload { progress }) => {
                let bar = LineGauge::default()
                    .filled_style(Style::new().white().on_light_magenta().bold())
                    .unfilled_style(Style::new().gray().on_black())
                    .ratio(f64::from(progress) / 100.0)
                    .filled_symbol(symbols::line::THICK_HORIZONTAL)
                    .unfilled_symbol(symbols::line::THICK_HORIZONTAL);
                bar.render(area.centered_vertically(Constraint::Length(1)), buf);
                return;
            }
            Some(DownloadState::ReleaseDownloadResult(Ok(_))) => {
                let text = "Download Sucessful".to_text().centered();
                let info = "Press `Enter` to restart or `Esc` to return"
                    .to_text()
                    .centered();
                text.render(area.centered_vertically(Constraint::Length(2)), buf);
                info.render(area.centered_vertically(Constraint::Length(1)), buf);
                return;
            }
            Some(DownloadState::ReleaseDownloadResult(Err(ref err))) => {
                let text = err.to_string();
                let text = text.to_text().centered();
                text.render(area.centered_vertically(Constraint::Length(1)), buf);
                return;
            }
            _ => {}
        }
        let releases = state.releases.iter().map(|release| {
            let mut line = Line::from_iter([
                release.name().to_string(),
                " - ".to_string(),
                release.version().to_string(),
            ]);
            if release.version() == self_update::cargo_crate_version!() {
                let text = "CURRENT".to_text();
                let padding = {
                    let total_width = area.width as usize;
                    let total_text_length = line.width() + text.width();
                    if total_text_length > total_width {
                        0
                    } else {
                        total_width - total_text_length
                    }
                };
                line.push_span(format!("{}{}", " ".repeat(padding), text));
            }
            line
        });
        let list = List::new(releases).highlight_style(Modifier::REVERSED);
        StatefulWidget::render(list, area, buf, &mut state.list_state);
    }
}

#[derive(Debug, serde::Deserialize)]
struct AssetResult {
    browser_download_url: String,
    #[serde(flatten)]
    _rest: HashMap<String, serde_json::Value>,
}
