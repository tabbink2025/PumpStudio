/* ------------------------------------------------------------------ */
/*  Typed RPC schema between the Bun process and the webview            */
/*                                                                      */
/*  Plain { requests, messages } shapes (what Electrobun's RPCSchema     */
/*  resolves to). Do NOT import from "electrobun/*" here: this file is  */
/*  shared by the DOM and Bun programs.                                 */
/*                                                                      */
/*  bun.requests      handled by Bun, called by the view                */
/*  bun.messages      received by Bun, sent by the view                 */
/*  webview.requests  handled by the view, called by Bun                */
/*  webview.messages  received by the view, pushed by Bun               */
/* ------------------------------------------------------------------ */

import type {
	AnalysisPayload,
	AnalysisStats,
	ConfigPatch,
	DataPoint,
	KeyCheck,
	MarketTab,
	MarketToken,
	OrchestratorResult,
	PublicConfig,
	RegisteredKey,
	SubmitResult,
	TrainingState,
	TrainingTarget,
	XpState,
} from "./types";

export type ViewId = "tokens" | "analyze" | "auto" | "leaderboard" | "settings";

export const VIEW_IDS: readonly ViewId[] = ["tokens", "analyze", "auto", "leaderboard", "settings"];

export type AppCommand =
	| { type: "navigate"; view: ViewId }
	| { type: "openPalette" }
	| { type: "refresh" }
	| { type: "toggleSidebar" };

export type PumpStudioRPC = {
	bun: {
		requests: {
			getMarket: { params: { tab: MarketTab; limit?: number }; response: MarketToken[] };
			getDataPoint: { params: { mint: string }; response: DataPoint };
			getAnalysisStats: { params: { limit?: number }; response: AnalysisStats };
			submitAnalysis: { params: AnalysisPayload; response: SubmitResult };
			orchestrateAnalysis: { params: { dataPoint: DataPoint }; response: OrchestratorResult };
			loadConfig: { params: void; response: PublicConfig };
			saveConfig: { params: ConfigPatch; response: PublicConfig };
			checkApiKey: { params: { apiKey?: string }; response: KeyCheck };
			registerApiKey: { params: { name: string; description?: string }; response: RegisteredKey };
			getTrainingState: { params: void; response: TrainingState };
			startAutoTraining: { params: { targets: TrainingTarget[] }; response: TrainingState };
			stopAutoTraining: { params: void; response: TrainingState };
			getXp: { params: void; response: XpState };
			openExternal: { params: { url: string }; response: boolean };
			openWebApp: { params: { path?: string }; response: boolean };
		};
		messages: {
			viewReady: void;
		};
	};
	webview: {
		requests: Record<never, never>;
		messages: {
			trainingState: TrainingState;
			xp: XpState;
			command: AppCommand;
		};
	};
};

export type BunRequests = PumpStudioRPC["bun"]["requests"];
export type WebviewMessages = PumpStudioRPC["webview"]["messages"];
