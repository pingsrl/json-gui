import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { useJsonStore } from "./store";

if (import.meta.env.DEV) {
  (window as any).__jsonStore = useJsonStore;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
