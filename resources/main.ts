import { AsyncContext, FocusHandle, View } from "gpui";
import { v_flex, h_flex, VirtualListScrollHandle, Scrollbar, InputState, Input, } from "gpui-base";
import { Props } from "gpui-shell";
import { theme as py_theme } from 'py-theme';

type Arg = {
    name: string,
}

type Command = {
    name: string,
    args: Arg[]
};

export default class Foo extends View {
    commands: Command[] = [
        {
            name: "close",
            args: [ { name: "session-id" } ]
        },
        { name: "open_lua", args: [] },
    ];
    scroll_handle = VirtualListScrollHandle.new();
    selected: null | number = null;
    focus: null | FocusHandle = null;
    query = InputState.new({ placeholder: "Search.." });
    filtered_commands: null | Command[] = null;

    init(_: Props, cx: AsyncContext): void {
        cx.bind_keys([
            { keystroke: "down", action: "next" },
            { keystroke: "up", action: "prev" },
        ]);
        this.query.on("change", (_, _cx) => {
            /*let value = this.query.value();
            let res = this.matcher.search(value);
            this.filtered_commands = res.map(item => {
                return item.item;
            });*/
            this.selected = 0;
            cx.notify();
        });
        this.focus = cx.focus_handle();
    }

    render(_: AsyncContext) {
        let commands = this.filtered_commands ?? this.commands;
        let theme = py_theme();
        return v_flex()
            .id("scrollbar")
            .size_full()
            .p_2()
            .gap_2()
            .on_action("focus-in", (_, cx) => {
                this.focus?.focus();
            })
            .on_action("next", (_, cx) => {
                if(this.selected === null) {
                    this.selected = 0;
                }else {
                    this.selected = Math.min((this.selected + 1), commands.length - 1);
                }
                cx.notify();
            })
            .on_action("prev", (_, cx) => {
                if(this.selected === null) {
                    this.selected = commands.length - 1;
                }else {
                    this.selected = Math.max(this.selected - 1, 0);
                }
                cx.notify();
            })
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Input.new(this.query))
            )
            .children(
                commands.map((cmd, i) => {
                    let selected = i === this.selected;
                    return h_flex()
                        .border_1()
                        .border_color(selected ? theme.selected_border : theme.border)
                        .rounded_sm()
                        .justify_between()
                        .when(selected, (self) => {
                            return self.bg(theme.selected);
                        })
                        .px_1()
                        .child(cmd_label(cmd));
                })
            )
            .child(Scrollbar.vertical("scrollbar"))
            .track_scroll(this.scroll_handle)
            .map((self) => {
                if(this.focus) {
                    return self.track_focus(this.focus);
                }else {
                    return self;
                }
            });
    }
}

function cmd_label(cmd: Command): string {
    let name = cmd.name + (cmd.args.length === 0 ? "" : "[");
    cmd.args.forEach((arg, i) => {
        name += arg.name;
        if(i !== cmd.args.length - 1) {
            name += ", ";
        }
    });
    name += (cmd.args.length === 0 ? "" : "]");
    return name;
}
