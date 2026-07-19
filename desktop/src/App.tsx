import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { TitleBar } from "@/components/TitleBar";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

function App() {
  const [greetMsg, setGreetMsg] = useState("");
  const [name, setName] = useState("");

  async function greet() {
    setGreetMsg(await invoke("greet", { name }));
  }

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <TitleBar />

      <main className="flex min-h-0 flex-1 items-center justify-center p-8">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <CardTitle className="text-2xl">Boris Desktop</CardTitle>
            <CardDescription>
              Tauri v2 + React + Tailwind + shadcn. Shell scaffold — engine
              wiring comes next.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form
              className="flex flex-col gap-4"
              onSubmit={(e) => {
                e.preventDefault();
                void greet();
              }}
            >
              <div className="flex flex-col gap-2">
                <Label htmlFor="greet-input">Name</Label>
                <Input
                  id="greet-input"
                  value={name}
                  onChange={(e) => setName(e.currentTarget.value)}
                  placeholder="Enter a name..."
                />
              </div>
              <Button type="submit" className="w-full">
                Greet from Rust
              </Button>
              {greetMsg ? (
                <p className="text-center text-sm text-muted-foreground">
                  {greetMsg}
                </p>
              ) : null}
            </form>
          </CardContent>
        </Card>
      </main>
    </div>
  );
}

export default App;
