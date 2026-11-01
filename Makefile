.PHONY: help install test compile run scheduler docker-up docker-down logs clean status

help:
	@echo "AETHER Python Commands"
	@echo "====================="
	@echo "make install    - Install Python dependencies"
	@echo "make test       - Run Python tests"
	@echo "make compile    - Compile-check Python files"
	@echo "make run        - Start FastAPI gateway"
	@echo "make scheduler  - Start standalone scheduler API"
	@echo "make docker-up  - Start Docker Compose stack"
	@echo "make docker-down- Stop Docker Compose stack"
	@echo "make logs       - Follow Docker Compose logs"
	@echo "make clean      - Remove Python caches"
	@echo "make status     - Show Docker Compose status"

install:
	pip install -r requirements.txt

test:
	python -m pytest tests_python

compile:
	python -m compileall aether tests_python

run:
	uvicorn aether.gateway:app --host 0.0.0.0 --port 8080

scheduler:
	uvicorn aether.scheduler_api:app --host 0.0.0.0 --port 50052

docker-up:
	docker compose up --build

docker-down:
	docker compose down

logs:
	docker compose logs -f

status:
	docker compose ps

clean:
	python -c "import pathlib, shutil; [shutil.rmtree(p) for p in pathlib.Path('.').rglob('__pycache__')]; shutil.rmtree('.pytest_cache', ignore_errors=True)"
