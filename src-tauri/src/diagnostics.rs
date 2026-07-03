use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};

#[derive(Clone, Debug)]
pub(crate) struct BackendLog {
    path: Arc<PathBuf>,
}

impl BackendLog {
    pub(crate) fn from_app(app: &AppHandle) -> Result<Self, String> {
        let path = backend_log_path(app)?;
        ensure_log_file(&path)?;
        Ok(Self {
            path: Arc::new(path),
        })
    }

    pub(crate) fn write(&self, level: &str, target: &str, message: impl AsRef<str>) {
        let _ = append_log_line(&self.path, level, target, message.as_ref());
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

pub(crate) fn initialize_backend_log(app: &AppHandle) -> Result<(), String> {
    let log = BackendLog::from_app(app)?;
    log.write("INFO", "backend", "InsituTranslate backend session started");
    Ok(())
}

pub(crate) fn open_backend_console(app: &AppHandle) -> Result<(), String> {
    let log = BackendLog::from_app(app)?;
    log.write("INFO", "diagnostics", "Opening backend console");
    open_backend_log_tail(log.path())
}

fn backend_log_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Unable to resolve app data directory: {error}"))?;
    Ok(app_data.join("logs").join("backend.log"))
}

fn ensure_log_file(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Unable to create diagnostics log directory: {error}"))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("Unable to open diagnostics log file: {error}"))?;
    Ok(())
}

fn append_log_line(path: &Path, level: &str, target: &str, message: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("Unable to write diagnostics log: {error}"))?;
    let clean_message = message.replace('\r', "\\r").replace('\n', "\\n");
    writeln!(
        file,
        "[{}] [{level}] [{target}] {clean_message}",
        timestamp()
    )
    .map_err(|error| format!("Unable to write diagnostics log: {error}"))
}

fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", now.as_secs(), now.subsec_millis())
}

#[cfg(windows)]
fn open_backend_log_tail(path: &Path) -> Result<(), String> {
    let script_path = write_tail_script(path)?;
    let shell = preferred_powershell();
    if windows_terminal_available() && open_windows_terminal_tail(&shell, &script_path).is_ok() {
        return Ok(());
    }
    open_legacy_powershell_tail(&shell, &script_path)
}

#[cfg(windows)]
fn open_windows_terminal_tail(shell: &str, script_path: &Path) -> std::io::Result<()> {
    use std::process::Command;

    Command::new("wt.exe")
        .args([
            "new-tab",
            "--title",
            "InsituTranslate 后端控制台",
            shell,
            "-NoLogo",
            "-NoExit",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script_path.to_string_lossy().as_ref(),
        ])
        .spawn()?;
    Ok(())
}

#[cfg(windows)]
fn open_legacy_powershell_tail(shell: &str, script_path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NEW_CONSOLE: u32 = 0x00000010;

    Command::new(shell)
        .args([
            "-NoLogo",
            "-NoExit",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script_path.to_string_lossy().as_ref(),
        ])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|error| format!("Unable to open backend console: {error}"))?;

    Ok(())
}

#[cfg(not(windows))]
fn open_backend_log_tail(_path: &Path) -> Result<(), String> {
    Err("Backend console is only implemented for Windows PowerShell".into())
}

#[cfg(windows)]
fn windows_terminal_available() -> bool {
    command_available("wt.exe")
}

#[cfg(windows)]
fn preferred_powershell() -> String {
    if command_available("pwsh.exe") {
        "pwsh.exe".to_string()
    } else {
        "powershell.exe".to_string()
    }
}

#[cfg(windows)]
fn command_available(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|directory| directory.join(name).is_file()))
        .unwrap_or(false)
}

#[cfg(windows)]
fn write_tail_script(log_path: &Path) -> Result<PathBuf, String> {
    let script_path = log_path
        .parent()
        .ok_or_else(|| "Unable to resolve diagnostics log directory".to_string())?
        .join("backend-console.ps1");
    fs::write(&script_path, build_tail_script(log_path))
        .map_err(|error| format!("Unable to write backend console script: {error}"))?;
    Ok(script_path)
}

#[cfg(windows)]
fn build_tail_script(path: &Path) -> String {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Continue'
chcp 65001 > $null
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

function Set-InsituConsoleFont {
    try {
        $source = @"
using System;
using System.Runtime.InteropServices;

[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
public struct CONSOLE_FONT_INFOEX {
    public uint cbSize;
    public uint nFont;
    public short dwFontSizeX;
    public short dwFontSizeY;
    public int FontFamily;
    public int FontWeight;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
    public string FaceName;
}

public static class InsituConsoleFont {
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr GetStdHandle(int nStdHandle);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern bool SetCurrentConsoleFontEx(
        IntPtr hConsoleOutput,
        bool bMaximumWindow,
        ref CONSOLE_FONT_INFOEX lpConsoleCurrentFontEx
    );

    public static bool Set(string faceName) {
        IntPtr handle = GetStdHandle(-11);
        CONSOLE_FONT_INFOEX info = new CONSOLE_FONT_INFOEX();
        info.cbSize = (uint)Marshal.SizeOf(typeof(CONSOLE_FONT_INFOEX));
        info.dwFontSizeY = 18;
        info.FontFamily = 54;
        info.FontWeight = 400;
        info.FaceName = faceName;
        return SetCurrentConsoleFontEx(handle, false, ref info);
    }
}
"@
        if (-not ('InsituConsoleFont' -as [type])) {
            Add-Type -TypeDefinition $source -ErrorAction Stop
        }
        foreach ($font in @('Cascadia Mono', 'Consolas')) {
            try {
                if ([InsituConsoleFont]::Set($font)) { return }
            } catch {}
        }
    } catch {}
}

function Write-InsituMessage {
    param(
        [Parameter(Mandatory = $true)][string]$Text,
        [Parameter(Mandatory = $true)][ConsoleColor]$DefaultColor
    )

    if ($Text -match 'HTTP 429|RESOURCE_EXHAUSTED|rate_limited=true') {
        Write-Host $Text -ForegroundColor Red
    } elseif ($Text -match 'will retry|retry_after_ms|Rate limit') {
        Write-Host $Text -ForegroundColor Yellow
    } else {
        Write-Host $Text -ForegroundColor $DefaultColor
    }
}

function Write-InsituLogLine {
    param([Parameter(Mandatory = $true)][string]$Line)

    $match = [regex]::Match(
        $Line,
        '^\[(?<time>[^\]]+)\]\s+\[(?<level>[^\]]+)\]\s+\[(?<target>[^\]]+)\]\s+(?<message>.*)$'
    )
    if (!$match.Success) {
        Write-InsituMessage -Text $Line -DefaultColor Gray
        return
    }

    $level = $match.Groups['level'].Value
    $target = $match.Groups['target'].Value
    $message = $match.Groups['message'].Value
    $levelColor = switch ($level) {
        'ERROR' { 'Red' }
        'WARN' { 'Yellow' }
        'INFO' { 'Cyan' }
        default { 'Gray' }
    }

    Write-Host '[' -NoNewline -ForegroundColor DarkGray
    Write-Host $match.Groups['time'].Value -NoNewline -ForegroundColor DarkGray
    Write-Host '] [' -NoNewline -ForegroundColor DarkGray
    Write-Host $level -NoNewline -ForegroundColor $levelColor
    Write-Host '] [' -NoNewline -ForegroundColor DarkGray
    Write-Host $target -NoNewline -ForegroundColor DarkCyan
    Write-Host '] ' -NoNewline -ForegroundColor DarkGray
    Write-InsituMessage -Text $message -DefaultColor Gray
}

try {
    Set-InsituConsoleFont
    $host.UI.RawUI.WindowTitle = 'InsituTranslate 后端控制台'
    $host.UI.RawUI.BackgroundColor = 'Black'
    $host.UI.RawUI.ForegroundColor = 'Gray'
} catch {}

$path = __LOG_PATH__
if (!(Test-Path -LiteralPath $path)) {
    New-Item -ItemType File -Force -Path $path | Out-Null
}

Clear-Host
Write-Host 'InsituTranslate 后端控制台' -ForegroundColor Cyan
Write-Host ('日志文件: ' + $path) -ForegroundColor DarkGray
Write-Host '高亮: ERROR 红色 / WARN 黄色 / INFO 青色；HTTP 429、rate_limited、RESOURCE_EXHAUSTED 会重点标出。' -ForegroundColor DarkGray
Write-Host '关闭窗口即可退出；Ctrl+C 可停止刷新。' -ForegroundColor DarkGray
Write-Host ''

Get-Content -LiteralPath $path -Encoding UTF8 -Tail 200 -Wait | ForEach-Object {
    Write-InsituLogLine -Line $_
}
"#;

    SCRIPT.replace(
        "__LOG_PATH__",
        &powershell_literal(path.to_string_lossy().as_ref()),
    )
}

#[cfg(windows)]
fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
