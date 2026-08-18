from __future__ import annotations

import httpx


class ModelClient:
    def __init__(self, base_url: str, api_key: str, model_name: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.model_name = model_name

    async def complete(self, prompt: str, *, tool_output: str | None = None) -> str:
        if not self.api_key:
            if tool_output:
                return f"Completed the requested task. Execution output:\n{tool_output}"
            return f"Dry-run response for: {prompt}"

        messages = [
            {
                "role": "system",
                "content": (
                    "You are a careful, concise coding agent. Explain what you did, do not claim "
                    "tools were used unless execution output is provided, and surface failures clearly."
                ),
            },
            {"role": "user", "content": prompt},
        ]
        if tool_output:
            messages.append({"role": "user", "content": f"Tool execution output:\n{tool_output}"})

        headers = {"Authorization": f"Bearer {self.api_key}"}
        payload = {"model": self.model_name, "messages": messages, "temperature": 0.2}
        async with httpx.AsyncClient(timeout=90) as client:
            response = await client.post(f"{self.base_url}/chat/completions", headers=headers, json=payload)
            response.raise_for_status()
            body = response.json()
        return body["choices"][0]["message"]["content"]
