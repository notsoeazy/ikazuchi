//! # Ikazuchi (雷) Design System & CSS Stylesheet
//!
//! Implements the mythological obsidian-and-lightning visual identity:
//! - `--ik-void: #08090E` (Deep storm-night obsidian canvas)
//! - `--ik-slate: #11131D` (Thunderhead elevated surface)
//! - `--ik-steel: #222638` (1px architectural borders)
//! - `--ik-thunder-gold: #F59E0B` (Imperial lightning gold primary accent)
//! - `--ik-raijin-indigo: #6366F1` (Electric indigo secondary accent)

pub const IKAZUCHI_CSS: &str = r#"
:root {
    --ik-void: #08090E;
    --ik-slate: #11131D;
    --ik-slate-hover: #181B29;
    --ik-steel: #222638;
    --ik-thunder-gold: #F59E0B;
    --ik-raijin-indigo: #6366F1;
    --ik-ivory: #F3F4F6;
    --ik-ash: #9CA3AF;
    --ik-crimson: #EF4444;
    --ik-emerald: #10B981;
}

* {
    box-sizing: border-box;
    margin: 0;
    padding: 0;
}

body, html {
    background-color: var(--ik-void);
    color: var(--ik-ivory);
    font-family: 'Inter', 'Noto Sans JP', system-ui, -apple-system, sans-serif;
    height: 100vh;
    width: 100vw;
    overflow: hidden;
    user-select: none;
}

.ik-shell {
    display: flex;
    height: 100vh;
    width: 100vw;
    background: var(--ik-void);
}

/* Left Command Rail (Desktop) */
.ik-rail {
    width: 230px;
    background: var(--ik-slate);
    border-right: 1px solid var(--ik-steel);
    display: flex;
    flex-direction: column;
    padding: 20px 12px;
    gap: 8px;
    flex-shrink: 0;
}

.ik-brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px 20px 12px;
    border-bottom: 1px solid var(--ik-steel);
    margin-bottom: 12px;
}

.ik-brand-kanji {
    font-size: 24px;
    font-weight: 900;
    color: var(--ik-thunder-gold);
    letter-spacing: 2px;
}

.ik-brand-title {
    font-size: 15px;
    font-weight: 800;
    letter-spacing: 3px;
    color: var(--ik-ivory);
}

.ik-nav-btn {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 11px 14px;
    background: transparent;
    color: var(--ik-ash);
    border: 1px solid transparent;
    border-radius: 4px;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 1px;
    text-transform: uppercase;
    cursor: pointer;
    text-align: left;
    transition: all 0.15s ease;
}

.ik-nav-btn:hover {
    background: var(--ik-slate-hover);
    color: var(--ik-ivory);
}

.ik-nav-btn.active {
    background: rgba(245, 158, 11, 0.1);
    color: var(--ik-thunder-gold);
    border-color: rgba(245, 158, 11, 0.35);
    border-left: 3px solid var(--ik-thunder-gold);
}

.ik-main {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    padding: 28px 32px;
}

.ik-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 24px;
    padding-bottom: 16px;
    border-bottom: 1px solid var(--ik-steel);
}

.ik-title {
    font-size: 22px;
    font-weight: 800;
    letter-spacing: 1.5px;
    text-transform: uppercase;
}

.ik-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(165px, 1fr));
    gap: 18px;
}

.ik-card {
    background: var(--ik-slate);
    border: 1px solid var(--ik-steel);
    border-radius: 4px;
    overflow: hidden;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    transition: border-color 0.15s ease, transform 0.15s ease;
}

.ik-card:hover {
    border-color: var(--ik-thunder-gold);
    transform: translateY(-2px);
}

.ik-card-cover {
    aspect-ratio: 2 / 3;
    background: #161926;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--ik-ash);
    font-size: 12px;
}

.ik-badge-unread {
    position: absolute;
    top: 8px;
    right: 8px;
    background: var(--ik-thunder-gold);
    color: #000;
    font-size: 11px;
    font-weight: 800;
    padding: 2px 7px;
    border-radius: 3px;
    font-variant-numeric: tabular-nums;
}

.ik-card-body {
    padding: 10px 12px;
}

.ik-card-title {
    font-size: 13px;
    font-weight: 700;
    color: var(--ik-ivory);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
}

.ik-card-sub {
    font-size: 11px;
    color: var(--ik-ash);
    margin-top: 4px;
}

/* Raijin Reader Continuous Webtoon Container */
.raijin-viewport {
    position: fixed;
    inset: 0;
    background: #040508;
    z-index: 100;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    align-items: center;
}

.raijin-webtoon-strip {
    display: flex;
    flex-direction: column;
    gap: 0;
    line-height: 0;
    margin: 0 auto;
}

.raijin-webtoon-strip img {
    display: block;
    width: 100%;
    height: auto;
    margin: 0;
    padding: 0;
}

.ik-btn-gold {
    background: var(--ik-thunder-gold);
    color: #08090E;
    border: none;
    border-radius: 4px;
    padding: 8px 16px;
    font-size: 12px;
    font-weight: 800;
    letter-spacing: 1px;
    text-transform: uppercase;
    cursor: pointer;
}

@media (max-width: 840px) {
    .ik-shell {
        flex-direction: column-reverse;
    }
    .ik-rail {
        width: 100%;
        height: 62px;
        flex-direction: row;
        justify-content: space-around;
        padding: 6px;
        border-right: none;
        border-top: 1px solid var(--ik-steel);
    }
    .ik-brand {
        display: none;
    }
}
"#;
