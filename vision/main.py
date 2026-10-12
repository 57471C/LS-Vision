from fastapi import FastAPI
import uvicorn

app = FastAPI(title="LS-Vision Sidecar")

@app.get("/health")
def health():
    return {"status": "ok", "engine": "python-sidecar"}

if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8008)