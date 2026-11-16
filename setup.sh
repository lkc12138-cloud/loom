#!/bin/sh
set -eu
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

echo "[1/4] 检查编译环境…"
if ! command -v cc >/dev/null 2>&1; then
    if [ "$(uname -s)" = "Darwin" ]; then
        echo "需要 C 编译器（Xcode Command Line Tools）。请运行："
        echo "  xcode-select --install"
        echo "安装完成后重新执行 ./setup.sh"
    else
        echo "需要 C 编译器（cc / gcc）。请先安装 build-essential 或 gcc 后重新执行 ./setup.sh"
    fi
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    if [ -t 0 ] && [ -t 1 ] && command -v curl >/dev/null 2>&1; then
        printf "未检测到 Rust 工具链。是否立即通过官方 rustup 安装？[Y/n] "
        read -r answer
        case "${answer:-Y}" in
            n|N|no|NO)
                echo "请访问 https://rustup.rs/ 手动安装 Rust 工具链后重试。"
                exit 1
                ;;
            *)
                curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
                export PATH="$HOME/.cargo/bin:$PATH"
                ;;
        esac
    else
        echo "需要 Rust 工具链：https://rustup.rs/ 。安装后重新执行 ./setup.sh"
        exit 1
    fi
fi

echo "[2/4] 编译 Loom 本机网关 (cargo build --release --locked)…"
cargo build --release --locked

echo "[3/4] 安装服务与配置模型厂商…"
./target/release/loom install "$@"

NO_START=0
for arg in "$@"; do
    if [ "$arg" = "--no-start" ]; then
        NO_START=1
    fi
done

echo "[4/4] 部署桌面端与完成指引…"
if [ "$(uname -s)" = "Darwin" ] && [ "$NO_START" -eq 0 ]; then
    ./target/release/loom app --build-only || echo "提示：Loom.app 构建未完成，可稍后运行 loom app 或使用 loom console。"
fi

case ":${PATH}:" in
    *":$HOME/.local/bin:"*) ;;
    *)
        PROFILE="~/.zshrc"
        case "${SHELL:-}" in
            *bash*) PROFILE="~/.bashrc" ;;
        esac
        echo "提示：检测到 ~/.local/bin 尚未加入 PATH，可执行以下命令使其全局生效："
        echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> $PROFILE && source $PROFILE"
        ;;
esac

echo ""
echo "全部就绪！你可以通过以下任意方式管理模型与监控调用："
if [ "$(uname -s)" = "Darwin" ] && [ "$NO_START" -eq 0 ]; then
    echo "  桌面端：打开 ~/Applications/Loom.app（或在菜单栏点击 ⌘）"
fi
echo "  网页端：运行 loom console（或在 Loom.app 中直接打开）"
echo "  终端端：直接运行 loom 进入交互菜单，或 loom doctor --ping 检测连通性"
