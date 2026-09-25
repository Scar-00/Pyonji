import { AsyncContext, Element, FocusHandle, View, svg } from "gpui";
import { Props } from "gpui-shell";
import { h_flex, Input, InputState, v_flex } from "gpui-base";
import { theme as py_theme } from "py-theme";

type Arg = {
    name: string,
}

type Command = {
    name: string,
    args: Arg[],
    description: string,
}

type Theme = ReturnType<typeof py_theme>;

export default class Foo extends View {
    commands: Command[] = [
        {
            name: "close",
            args: [{ name: "session-id" }],
            description: "Close the active terminal session",
        },
        {
            name: "open_lua",
            args: [],
            description: "Switch to the Lua workspace",
        },
    ];
    selected: null | number = null;
    focus: null | FocusHandle = null;
    query = InputState.new({ placeholder: "Search commands…" });

    init(_: Props, cx: AsyncContext): void {
        cx.bind_keys([
            { keystroke: "down", action: "next" },
            { keystroke: "up", action: "prev" },
        ]);
        this.query.on("change", (_, _cx) => {
            this.selected = this.visible_commands().length > 0 ? 0 : null;
            cx.notify();
        });
        this.focus = cx.focus_handle();
    }

    visible_commands(): Command[] {
        let query = this.query.value().trim().toLowerCase();
        if(query.length === 0) {
            return this.commands;
        }

        return this.commands.filter((command) => {
            let searchable = [
                command.name,
                command.description,
                ...command.args.map((arg) => arg.name),
            ].join(" ").toLowerCase();
            return searchable.includes(query);
        });
    }

    render(_: AsyncContext) {
        let commands = this.visible_commands();
        let theme = py_theme();
        let selected = this.selected !== null && this.selected < commands.length
            ? this.selected
            : null;

        return v_flex()
            .id("command-palette")
            .size_full()
            .rounded_lg()
            .shadow_xl()
            .overflow_hidden()
            .on_action("focus-in", (_, cx) => {
                this.focus?.focus();
            })
            .on_action("next", (_, cx) => {
                if(commands.length === 0) {
                    this.selected = null;
                }else if(this.selected === null) {
                    this.selected = 0;
                }else {
                    this.selected = Math.min(this.selected + 1, commands.length - 1);
                }
                cx.notify();
            })
            .on_action("prev", (_, cx) => {
                if(commands.length === 0) {
                    this.selected = null;
                }else if(this.selected === null) {
                    this.selected = commands.length - 1;
                }else {
                    this.selected = Math.max(this.selected - 1, 0);
                }
                cx.notify();
            })
            .child(this.render_search(theme))
            .child(
                commands.length === 0
                    ? this.render_empty_state(theme)
                    : v_flex()
                        .flex_1()
                        .min_h_0()
                        .p_2()
                        .gap_1()
                        .overflow_y_scrollbar()
                        .role("list")
                        .children(commands.map((command, index) => {
                            return this.render_command(command, index, selected === index, theme);
                        })),
            )
            .map((self) => {
                if(this.focus) {
                    return self.track_focus(this.focus);
                }
                return self;
            });
    }

    render_search(theme: Theme): Element {
        return v_flex()
            .w_full()
            .p_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .w_full()
                    .h(42)
                    .px_3()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .w(20)
                            .items_center()
                            .justify_center()
                            .text_size(18)
                            .text_color(theme.text_muted)
                            .child("⌕"),
                    )
                    .child(
                        Input.new(this.query)
                            .flex_1()
                            .border_0()
                            .rounded(0)
                            .bg(theme.background)
                            .text_color(theme.text),
                    )
                    .child(
                        h_flex()
                            .h(22)
                            .px_2()
                            .items_center()
                            .rounded_sm()
                            .bg(theme.surface)
                            .border_1()
                            .border_color(theme.border)
                            .text_xs()
                            .font_medium()
                            .text_color(theme.text_disabled)
                            .child("esc"),
                    ),
            );
    }

    render_command(command: Command, index: number, selected: boolean, theme: Theme): Element {
        return h_flex()
            .id(`command-${index}`)
            .w_full()
            .h(58)
            .px_3()
            .items_center()
            .gap_3()
            .rounded_md()
            .border_l_2()
            .border_color(selected ? theme.selected_border : theme.unselected_border)
            .bg(selected ? theme.selected : theme.surface)
            .hover((self) => {
                if(!selected) {
                    self.bg(theme.hovered);
                }
            })
            .cursor_pointer()
            .on_click((_, cx) => {
                this.selected = index;
                cx.notify();
            })
            .role("list_item")
            .aria_selected(selected)
            .map((self) => selected ? self.aria_active_descendant() : self)
            .set_position(index + 1, this.visible_commands().length)
            .child(
                h_flex()
                    .size(30)
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(selected ? theme.accent_muted : theme.surface_elevated)
                    .border_1()
                    .border_color(selected ? theme.accent : theme.border)
                    .text_color(selected ? theme.accent : theme.text_muted)
                    .child(
                        command.name === "open_lua"
                            ? svg("icons/lua.svg").size(15)
                            : h_flex().text_size(18).font_medium().child("×"),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(2)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(theme.text)
                                    .whitespace_nowrap()
                                    .child(command.name),
                            )
                            .children(
                                command.args.map((arg) => {
                                    return h_flex()
                                        .h(20)
                                        .px_1p5()
                                        .items_center()
                                        .rounded_sm()
                                        .bg(theme.background)
                                        .border_1()
                                        .border_color(theme.border)
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .whitespace_nowrap()
                                        .child(arg.name);
                                }),
                            ),
                    )
                    .child(
                        h_flex()
                            .text_xs()
                            .text_color(selected ? theme.text_muted : theme.text_disabled)
                            .truncate()
                            .child(command.description),
                    ),
            )
            .child(
                h_flex()
                    .w(24)
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(selected ? theme.accent : theme.text_disabled)
                    .child(selected ? "↵" : "›"),
            );
    }

    render_empty_state(theme: Theme): Element {
        return v_flex()
            .flex_1()
            .min_h_0()
            .px_4()
            .py_5()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                h_flex()
                    .size(42)
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.border)
                    .text_size(20)
                    .text_color(theme.text_disabled)
                    .child("⌕"),
            )
            .child(
                h_flex()
                    .text_sm()
                    .font_semibold()
                    .text_color(theme.text)
                    .child("No matching commands"),
            )
            .child(
                h_flex()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("Try a different search term"),
            );
    }
}
