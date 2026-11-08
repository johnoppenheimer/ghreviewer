import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // Keep Rust errors visible in the terminal.
  clearScreen: false,
  // Tauri's devUrl expects this exact port.
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
});
