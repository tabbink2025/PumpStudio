import type { ElectrobunConfig } from "electrobun";

export default {
	app: {
		name: "Pump.studio",
		identifier: "studio.pump.app",
		version: "0.2.0",
		description: "Pump.studio desktop: token browser and agent trainer",
	},
	build: {
		// Bun main process -> Resources/app/bun/
		bun: { entrypoint: "src/bun/index.ts" },
		// The React UI is built by Vite (Tailwind/PostCSS), then copied in.
		// views://mainview/index.html === Resources/app/views/mainview/index.html
		copy: {
			"dist/index.html": "views/mainview/index.html",
			"dist/assets": "views/mainview/assets",
		},
		mac: { bundleCEF: false, icons: "icon.iconset" },
		linux: { bundleCEF: false },
		win: { bundleCEF: false },
	},
	runtime: { exitOnLastWindowClosed: true },
} satisfies ElectrobunConfig;
