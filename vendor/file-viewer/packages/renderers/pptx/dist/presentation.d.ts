import type { PptxViewer } from './viewer';
export interface PptxPresentationState {
    active: boolean;
    slideNumber: number;
    total: number;
}
export interface PptxPresentationLabels {
    exit?: string;
    hint?: string;
    next?: string;
    previous?: string;
}
/**
 * Full-screen slideshow for a rendered PPTX deck.
 *
 * The slides stay where the viewer put them: the whole scale box is moved into an overlay and the
 * inactive slots are hidden with CSS, so the engine's scoped `.flyfish-pptx-content .slide` rules
 * keep applying and no node is cloned. A placeholder marks the original position so exiting puts
 * everything back exactly where it was.
 */
export declare class PptxPresentation {
    private readonly viewer;
    private readonly labels;
    private readonly fullscreen;
    private overlay;
    private stage;
    private counter;
    private placeholder;
    private listeners;
    private layoutFrame;
    private ownsFullscreen;
    private current;
    constructor(viewer: PptxViewer, labels?: PptxPresentationLabels, fullscreen?: boolean);
    get active(): boolean;
    get slideNumber(): number;
    get state(): PptxPresentationState;
    enter(slideNumber?: number): Promise<void>;
    exit(): void;
    toggle(slideNumber?: number): Promise<void>;
    goTo(slideNumber: number): void;
    next(): void;
    previous(): void;
    /** Scale the active slide to fit the overlay, letterboxing whichever axis has slack. */
    layout(): void;
    destroy(): void;
    private slots;
    /**
     * The nodes the slideshow toggles between. Windowed decks wrap each slide in a
     * slot; the default (non-windowed) deck appends slides directly to the content
     * node, so those are the containers there.
     */
    private slideContainers;
    /**
     * Chromium reports the fullscreen element of a shadow root on the shadow root
     * itself while document.fullscreenElement stays on the host, so both have to
     * be checked before deciding who owns fullscreen.
     */
    private isFullscreenElement;
    private scheduleLayout;
    private notify;
    private on;
    private attach;
    private detach;
    private handleClick;
    private handleKey;
}
