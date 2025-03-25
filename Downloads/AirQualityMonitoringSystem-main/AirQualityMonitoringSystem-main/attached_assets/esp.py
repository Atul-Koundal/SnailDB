import requests
import matplotlib.pyplot as plt
import time
from datetime import datetime

ESP_IP = "http://192.168.137.91/data"  # Change this to match your ESP8266 IP

# Lists to store data for plotting
timestamps = []
temperatures = []
humidities = []
air_qualities = []

# Setup Matplotlib
plt.ion()
fig, ax = plt.subplots(3, 1, figsize=(8, 10))

def fetch_data():
    """Fetch sensor data from ESP8266"""
    try:
        response = requests.get(ESP_IP, timeout=5)
        data = response.json()
        return data
    except Exception as e:
        print(f"Error fetching data: {e}")
        return None

def update_plot():
    """Update the Matplotlib plot dynamically"""
    ax[0].cla()
    ax[1].cla()
    ax[2].cla()

    ax[0].plot(timestamps, temperatures, marker='o', color='r', label="Temperature (°C)")
    ax[1].plot(timestamps, humidities, marker='s', color='b', label="Humidity (%)")
    ax[2].plot(timestamps, air_qualities, marker='d', color='g', label="Air Quality (PPM)")

    ax[0].set_title("Temperature Over Time")
    ax[1].set_title("Humidity Over Time")
    ax[2].set_title("Air Quality Over Time")

    for i in range(3):
        ax[i].set_xlabel("Time")
        ax[i].legend()
        ax[i].grid(True)

    plt.tight_layout()
    plt.draw()
    plt.pause(1)

# Main loop to fetch and plot data every minute
while True:
    sensor_data = fetch_data()
    if sensor_data:
        current_time = datetime.now().strftime("%H:%M:%S")
        timestamps.append(current_time)
        temperatures.append(sensor_data["temperature"])
        humidities.append(sensor_data["humidity"])
        air_qualities.append(sensor_data["airQuality"])

        update_plot()

    time.sleep(3)  # Wait 1 minute before fetching new data
