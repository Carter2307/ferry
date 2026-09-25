# Debug/echo service used by Ferry's end-to-end tests.
FROM python:3.12-alpine
WORKDIR /app
COPY echo.py .
EXPOSE 8000
CMD ["python", "-u", "echo.py"]
