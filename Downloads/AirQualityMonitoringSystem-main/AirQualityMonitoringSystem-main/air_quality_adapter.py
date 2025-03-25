import requests
import time
from datetime import datetime
import random

# ESP8266 IP address - can be changed from the main app
ESP_IP = "http://192.168.137.91/data"

# Lists to store data for tracking
timestamps = []
temperatures = []
humidities = []
air_qualities = []

# Maximum number of data points to store
MAX_DATA_POINTS = 1000

# Base values for demo mode
base_temp = 24.5
base_humidity = 48.0
base_air_quality = 150

def fetch_data_from_esp():
    """Fetch sensor data directly from ESP8266"""
    try:
        response = requests.get(ESP_IP, timeout=5)
        data = response.json()
        return data, None
    except Exception as e:
        # Return an empty data structure instead of None to avoid "not subscriptable" errors
        return {
            "temperature": 0,
            "humidity": 0,
            "airQuality": 0
        }, f"Error fetching data from ESP8266: {e}"

def generate_sample_data():
    """Generate simulated data for demo mode"""
    global base_temp, base_humidity, base_air_quality
    
    # Generate slightly varying data around base values
    temp_variation = random.uniform(-0.5, 0.5)
    humidity_variation = random.uniform(-2.0, 2.0)
    air_quality_variation = random.uniform(-10, 10)
    
    # Update base values with some drift to simulate changing conditions
    base_temp += random.uniform(-0.1, 0.1)
    base_humidity += random.uniform(-0.2, 0.2)
    base_air_quality += random.uniform(-5, 5)
    
    # Keep values in realistic ranges
    base_temp = max(18, min(30, base_temp))
    base_humidity = max(30, min(70, base_humidity))
    base_air_quality = max(50, min(500, base_air_quality))
    
    # Return simulated data
    return {
        "temperature": base_temp + temp_variation,
        "humidity": base_humidity + humidity_variation,
        "airQuality": int(base_air_quality + air_quality_variation)
    }

def get_sensor_data(use_demo_mode=False, esp_ip=None):
    """Main function to get sensor data - either from ESP or demo mode"""
    global ESP_IP, timestamps, temperatures, humidities, air_qualities
    
    # Update ESP IP if specified
    if esp_ip:
        ESP_IP = esp_ip
    
    # Get data from appropriate source
    if use_demo_mode:
        data = generate_sample_data()
        error = None
    else:
        data, error = fetch_data_from_esp()
        # We'll still use the data even if there's an error because fetch_data_from_esp now returns 
        # a valid data structure with zeros instead of None
    
    # Store data in history
    current_time = datetime.now().strftime("%H:%M:%S")
    timestamps.append(current_time)
    temperatures.append(data["temperature"])
    humidities.append(data["humidity"])
    air_qualities.append(data["airQuality"])
    
    # Limit data points
    if len(timestamps) > MAX_DATA_POINTS:
        timestamps.pop(0)
        temperatures.pop(0)
        humidities.pop(0)
        air_qualities.pop(0)
    
    return data, error

def get_historical_data():
    """Get historical data for plotting"""
    return {
        "timestamps": timestamps,
        "temperatures": temperatures,
        "humidities": humidities,
        "air_qualities": air_qualities
    }

# Function to adapt the original esp.py fetch_data approach
def fetch_data():
    """Replicates the original esp.py fetch_data function"""
    data, error = fetch_data_from_esp()
    if error:
        print(error)
        return None
    return data

# This allows running the file directly for testing
if __name__ == "__main__":
    print("Starting ESP8266 data collection...")
    while True:
        data, error = get_sensor_data(use_demo_mode=True)
        if data:
            print(f"Temp: {data['temperature']:.1f}°C, Humidity: {data['humidity']:.1f}%, Air Quality: {data['airQuality']} PPM")
        else:
            print(f"Error: {error}")
        time.sleep(3)