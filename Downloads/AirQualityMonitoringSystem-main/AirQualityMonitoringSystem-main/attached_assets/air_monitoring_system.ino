#include <ESP8266WiFi.h>
#include <ESP8266WebServer.h>
#include <ArduinoJson.h>
#include <DHT.h>

// WiFi Credentials
const char* ssid = "agh";
const char* password = "blackniqqa";

// Pin Definitions
#define DHTPIN 4      // D2 on NodeMCU
#define DHTTYPE DHT11 // DHT 11
#define MQ135_PIN A0  // A0 on NodeMCU

// Initialize DHT sensor
DHT dht(DHTPIN, DHTTYPE);

// Initialize web server on port 80
ESP8266WebServer server(80);

// Variables for sensor readings
float temperature;
float humidity;
int airQuality;

void setup() {
  Serial.begin(115200);
  Serial.println("\nESP8266 Air Quality Monitoring System");

  dht.begin();

  // Connect to WiFi
  connectToWiFi();

  // Define the API endpoint
  server.on("/data", HTTP_GET, handleSensorData);

  server.begin();
  Serial.println("HTTP server started, waiting for requests...");
}

void loop() {
  server.handleClient();
}

// Function to connect to WiFi (runs indefinitely until connected)
void connectToWiFi() {
  Serial.print("Connecting to WiFi: ");
  Serial.println(ssid);

  WiFi.begin(ssid, password);

  // Keep trying until connected
  while (WiFi.status() != WL_CONNECTED) {
    delay(500);
    Serial.print(".");
  }

  Serial.println("\nWiFi Connected!");
  Serial.print("ESP8266 IP Address: ");
  Serial.println(WiFi.localIP());
}

// Function to read sensor values and log to Serial Monitor
void readSensors() {
  humidity = dht.readHumidity();
  temperature = dht.readTemperature();
  airQuality = analogRead(MQ135_PIN);

  if (isnan(humidity) || isnan(temperature)) {
    Serial.println("Error reading from DHT sensor!");
    humidity = humidity > 0 ? humidity : 50;
    temperature = temperature > 0 ? temperature : 25;
  }

  // Log to Serial Monitor
  Serial.println("===== Sensor Readings =====");
  Serial.print("Temperature: ");
  Serial.print(temperature);
  Serial.println(" °C");

  Serial.print("Humidity: ");
  Serial.print(humidity);
  Serial.println(" %");

  Serial.print("Air Quality: ");
  Serial.print(airQuality);
  Serial.println(" PPM");
}

// Handle HTTP request and send JSON response
void handleSensorData() {
  readSensors();  // Read latest sensor values

  StaticJsonDocument<200> jsonDoc;
  jsonDoc["temperature"] = temperature;
  jsonDoc["humidity"] = humidity;
  jsonDoc["airQuality"] = airQuality;

  String jsonString;
  serializeJson(jsonDoc, jsonString);

  server.send(200, "application/json", jsonString);
}
