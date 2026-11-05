/* ------------------------------------------------------------------ */
/*  Transport to the Bun process                                        */
/*                                                                      */
/*  Inside the desktop app this is Electrobun's typed RPC. In a plain   */
/*  browser (vite dev, screenshots, design work) it falls back to an    */
/*  in-memory demo backend with realistic generated data.               */
/* ------------------------------------------------------------------ */

import { Electroview } from "electrobun/view";
import type { BunRequests, PumpStudioRPC, WebviewMessages } from "../../shared/rpc-schema";
import { createDemoBackend } from "./demo";

/** Electrobun injects these globals before any page script runs. */
export const isDesktop: boolean =
	typeof window !== "undefined" && typeof window.__electrobunWebviewId === "number" && !!window.__electrobun;

/** Demo mode = no desktop bridge (or forced with ?demo). */
export const isDemo = !isDesktop || new URLSearchParams(location.search).has("demo");

type Listener<K extends keyof WebviewMessages> = (payload: WebviewMessages[K]) => void;
const listeners = new Map<keyof WebviewMessages, Set<(payload: never) => void>>();

function emit<K extends keyof WebviewMessages>(name: K, payload: WebviewMessages[K]) {
	listeners.get(name)?.forEach((fn) => (fn as Listener<K>)(payload));
}

/** Subscribe to a message pushed by the Bun process. Returns an unsubscribe function. */
export function on<K extends keyof WebviewMessages>(name: K, fn: Listener<K>): () => void {
	let set = listeners.get(name);
	if (!set) listeners.set(name, (set = new Set()));
	set.add(fn as (payload: never) => void);
	return () => {
		set!.delete(fn as (payload: never) => void);
	};
}

type Method = keyof BunRequests;
type Params<M extends Method> = BunRequests[M]["params"];
type Result<M extends Method> = BunRequests[M]["response"];
type RequestFn = <M extends Method>(method: M, params: Params<M>) => Promise<Result<M>>;

let request: RequestFn;
let sendViewReady: () => void = () => {};

if (isDesktop && !isDemo) {
	const rpc = Electroview.defineRPC<PumpStudioRPC>({
		// Default is 1000 ms. AI auto-fill can take a minute or more.
		maxRequestTime: 180_000,
		handlers: {
			requests: {},
			messages: {
				trainingState: (p) => emit("trainingState", p),
				xp: (p) => emit("xp", p),
				command: (p) => emit("command", p),
			},
		},
	});
	new Electroview({ rpc });
	request = rpc.request as unknown as RequestFn;
	sendViewReady = () => rpc.send.viewReady();
} else {
	const demo = createDemoBackend((name, payload) => emit(name, payload as never));
	request = demo.request as RequestFn;
}

/** Typed calls into the Bun process. */
export const bun = {
	getMarket: (p: Params<"getMarket">) => request("getMarket", p),
	getDataPoint: (mint: string) => request("getDataPoint", { mint }),
	getAnalysisStats: (limit = 25) => request("getAnalysisStats", { limit }),
	submitAnalysis: (p: Params<"submitAnalysis">) => request("submitAnalysis", p),
	orchestrateAnalysis: (p: Params<"orchestrateAnalysis">) => request("orchestrateAnalysis", p),
	loadConfig: () => request("loadConfig", undefined),
	saveConfig: (p: Params<"saveConfig">) => request("saveConfig", p),
	checkApiKey: (apiKey?: string) => request("checkApiKey", { apiKey }),
	registerApiKey: (name: string, description?: string) => request("registerApiKey", { name, description }),
	getTrainingState: () => request("getTrainingState", undefined),
	startAutoTraining: (p: Params<"startAutoTraining">) => request("startAutoTraining", p),
	stopAutoTraining: () => request("stopAutoTraining", undefined),
	getXp: () => request("getXp", undefined),
	openExternal: (url: string) => (isDesktop ? request("openExternal", { url }) : browserOpen(url)),
	openWebApp: (path?: string) =>
		isDesktop ? request("openWebApp", { path }) : browserOpen(`https://pump.studio${path ?? "/market"}`),
	viewReady: () => sendViewReady(),
};

/** window.open returns null when noopener is set, so report success explicitly. */
function browserOpen(url: string): Promise<boolean> {
	window.open(url, "_blank", "noopener,noreferrer");
	return Promise.resolve(true);
}

/** Normalize anything thrown across the bridge into a readable message. */
export function errorMessage(err: unknown): string {
	if (err instanceof Error) return err.message.replace(/^Error:\s*/, "");
	if (typeof err === "string") return err;
	return "Something went wrong";
}
