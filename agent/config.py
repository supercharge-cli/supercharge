from functools import lru_cache

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_prefix="", extra="ignore")

    host: str = Field(default="0.0.0.0", alias="HERMES_HOST")
    port: int = Field(default=8080, alias="HERMES_PORT")
    log_level: str = Field(default="INFO", alias="HERMES_LOG_LEVEL")
    database_url: str = Field(default="sqlite:///./runtime/hermes.db", alias="HERMES_DATABASE_URL")
    api_token: str = Field(default="", alias="HERMES_API_TOKEN")

    model_base_url: str = Field(default="https://api.openai.com/v1", alias="MODEL_BASE_URL")
    model_api_key: str = Field(default="", alias="MODEL_API_KEY")
    model_name: str = Field(default="gpt-4.1-mini", alias="MODEL_NAME")

    e2b_api_key: str = Field(default="", alias="E2B_API_KEY")
    e2b_template: str = Field(default="base", alias="E2B_TEMPLATE")
    e2b_timeout_seconds: int = Field(default=900, alias="E2B_TIMEOUT_SECONDS", ge=30, le=86400)

    max_tool_calls_per_turn: int = Field(default=12, alias="MAX_TOOL_CALLS_PER_TURN", ge=1, le=100)
    max_command_seconds: int = Field(default=60, alias="MAX_COMMAND_SECONDS", ge=1, le=900)
    max_output_bytes: int = Field(default=200_000, alias="MAX_OUTPUT_BYTES", ge=1_024, le=2_000_000)


@lru_cache
def get_settings() -> Settings:
    return Settings()
