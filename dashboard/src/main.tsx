import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./style.css";
import { createClient } from "./api";

const root = document.getElementById("root");
if (!root) throw new Error("the page has no #root to render into");

createRoot(root).render(
  <StrictMode>
    <App client={createClient()} />
  </StrictMode>,
);
