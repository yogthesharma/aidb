import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router";
import { AppTheme } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import App from "./App.tsx";
import "./index.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter>
      <AppTheme>
        <TooltipProvider delayDuration={350}>
          <App />
          <Toaster position="bottom-right" closeButton duration={3500} />
        </TooltipProvider>
      </AppTheme>
    </BrowserRouter>
  </StrictMode>,
);
