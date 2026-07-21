# gswitch completions for fish shell
# Install: copy to ~/.config/fish/completions/gswitch.fish
#          or /usr/share/fish/completions/gswitch.fish

# global options
complete -c gswitch -l dry-run -d "预览模式：仅显示将要执行的操作，不实际修改系统"
complete -c gswitch -l help -d "显示帮助信息"
complete -c gswitch -l version -d "显示版本信息"

# subcommands
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a integrated -d "切换到仅集成显卡（禁用 NVIDIA）"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a passthrough -d "将 NVIDIA GPU 绑定到 vfio-pci，直通给虚拟机"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a hybrid -d "PRIME 混合模式（按需渲染，省电）"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a nvidia -d "NVIDIA 独立显卡模式（高性能）"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a query -d "查询当前显卡模式"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a switchable -d "检查系统是否支持显卡切换"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a power -d "查询或控制运行时 GPU 电源"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a default -d "获取推荐的默认显卡模式"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a ext-display -d "检查外接显示器是否需要 NVIDIA 独立显卡"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a runtime-pm -d "检查 GPU 是否支持运行时电源管理"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a reset -d "重置所有 gswitch GPU 配置"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a cache-create -d "创建 NVIDIA GPU 缓存（需要混合或计算模式）"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a cache-delete -d "删除 NVIDIA GPU 缓存"
complete -c gswitch -f -n "not __fish_seen_subcommand_from integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query" \
    -a cache-query -d "查询 NVIDIA GPU 缓存内容"

# subcommand-specific options

# hybrid --rtd3
complete -c gswitch -f -n "__fish_seen_subcommand_from hybrid" \
    -l rtd3 -d "RTD3 电源管理级别 [0-3]"
complete -c gswitch -f -n "__fish_seen_subcommand_from hybrid; and __fish_contains_opt rtd3" \
    -a "0 1 2 3" -d "RTD3 级别"

# power subcommand actions
complete -c gswitch -f -n "__fish_seen_subcommand_from power; and not __fish_seen_subcommand_from on off auto" \
    -a on -d "开启 NVIDIA GPU"
complete -c gswitch -f -n "__fish_seen_subcommand_from power; and not __fish_seen_subcommand_from on off auto" \
    -a off -d "关闭 NVIDIA GPU"
complete -c gswitch -f -n "__fish_seen_subcommand_from power; and not __fish_seen_subcommand_from on off auto" \
    -a auto -d "自动配置电源（基于当前模式 + RTD3 支持能力）"