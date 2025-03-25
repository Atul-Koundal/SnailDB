import pandas as pd
import numpy as np
from datetime import datetime, timedelta
from sklearn.linear_model import LinearRegression
from sklearn.preprocessing import PolynomialFeatures
#matlab
# Temperature interpretation
def interpret_temperature(temp):
    if temp < 18:
        return "❄️ Cool"
    elif 18 <= temp <= 24:
        return "✅ Comfortable"
    elif 24 < temp <= 28:
        return "🌡️ Warm"
    else:
        return "🔥 Hot"

# Humidity interpretation
def interpret_humidity(humidity):
    if humidity < 30:
        return "🏜️ Dry"
    elif 30 <= humidity <= 50:
        return "✅ Comfortable"
    elif 50 < humidity <= 70:
        return "💧 Slightly Humid"
    else:
        return "💦 Very Humid"

# Air quality interpretation
def interpret_air_quality(air_quality):
    # These thresholds are approximations and may need adjustment
    # based on the specific gas being monitored by MQ135
    if air_quality < 100:
        return "✅ Excellent", "The air quality is excellent. The environment is clean and healthy."
    elif 100 <= air_quality < 200:
        return "🟢 Good", "The air quality is good. Most people won't experience negative health effects."
    elif 200 <= air_quality < 300:
        return "🟡 Moderate", "The air quality is moderate. Sensitive individuals may experience slight irritation."
    elif 300 <= air_quality < 400:
        return "🟠 Unhealthy for Sensitive Groups", "Members of sensitive groups may experience health effects. Consider improving ventilation."
    elif 400 <= air_quality < 500:
        return "🔴 Unhealthy", "Everyone may begin to experience health effects. Consider improving ventilation immediately."
    else:
        return "🟣 Hazardous", "Health alert: everyone may experience more serious health effects. Improve ventilation urgently."

# Generate predictions based on historical data using polynomial regression
def generate_predictions(df, predict_points=5, degree=2):
    # Convert to numerical indices for prediction
    X = np.array(range(len(df))).reshape(-1, 1)
    
    # Create polynomial features
    poly = PolynomialFeatures(degree=degree)
    X_poly = poly.fit_transform(X)
    
    # Create models for each metric
    temp_model = LinearRegression().fit(X_poly, df["Temperature (°C)"])
    humidity_model = LinearRegression().fit(X_poly, df["Humidity (%)"])
    air_quality_model = LinearRegression().fit(X_poly, df["Air Quality (PPM)"])
    
    # Generate future indices
    future_indices = np.array(range(len(df), len(df) + predict_points)).reshape(-1, 1)
    future_indices_poly = poly.transform(future_indices)
    
    # Predict future values
    future_temp = temp_model.predict(future_indices_poly)
    future_humidity = humidity_model.predict(future_indices_poly)
    future_air_quality = air_quality_model.predict(future_indices_poly)
    
    # Create prediction timestamps (continuing from last timestamp)
    last_time = datetime.strptime(df["Time"].iloc[-1], "%H:%M:%S")
    pred_times = [(last_time + timedelta(seconds=(i+1)*5)).strftime("%H:%M:%S") 
                   for i in range(predict_points)]
    
    # Create prediction dataframe
    pred_df = pd.DataFrame({
        "Time": pred_times,
        "Temperature (°C)": future_temp,
        "Humidity (%)": future_humidity,
        "Air Quality (PPM)": future_air_quality
    }).set_index("Time")
    
    return pred_df

# Get overall assessment of the environment
def get_overall_assessment(temp, humidity, air_quality):
    # Get individual assessments
    temp_status = interpret_temperature(temp)
    humidity_status = interpret_humidity(humidity)
    air_quality_status, _ = interpret_air_quality(air_quality)
    
    # Determine if any metric is in a concerning state
    temp_concern = temp < 16 or temp > 28
    humidity_concern = humidity < 20 or humidity > 70
    air_concern = air_quality >= 300
    
    # Build overall assessment
    if temp_concern and humidity_concern and air_concern:
        return """
        🚨 **Multiple Environmental Concerns**: The temperature, humidity, and air quality are all 
        outside ideal ranges. This environment may be uncomfortable and potentially harmful 
        to health over extended periods. Consider adjusting ventilation, temperature control, 
        and air cleaning.
        """
    elif (temp_concern and humidity_concern) or (temp_concern and air_concern) or (humidity_concern and air_concern):
        return """
        ⚠️ **Some Environmental Concerns**: Multiple environmental factors are outside ideal ranges.
        This may affect comfort and potentially health. Review the individual measurements and 
        consider making appropriate adjustments to improve the environment.
        """
    elif temp_concern:
        message = "too cold" if temp < 16 else "too hot"
        return f"""
        🌡️ **Temperature Concern**: The environment is {message}. While other factors are within
        acceptable ranges, the temperature may cause discomfort. Consider adjusting heating or cooling
        as appropriate.
        """
    elif humidity_concern:
        message = "too dry" if humidity < 20 else "too humid"
        return f"""
        💧 **Humidity Concern**: The environment is {message}. This may cause discomfort such as 
        dry skin/throat or excess moisture/condensation. Consider using a humidifier or dehumidifier
        as appropriate.
        """
    elif air_concern:
        return """
        🌬️ **Air Quality Concern**: The air quality readings suggest contamination above recommended
        levels. This may cause respiratory irritation or other health effects. Consider improving
        ventilation, identifying pollution sources, or using air purification.
        """
    else:
        return """
        ✅ **Good Environmental Conditions**: All measured parameters are within comfortable ranges.
        The current environment should feel comfortable and be healthy for most people.
        """
